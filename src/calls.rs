//! Голосовая комната как в Discord: приглашение → «Войти» → разговор на N.
//!
//! Комната — это mesh-сессия: каждый участник соединён с каждым и слышит всех.
//! «Позвонить» и «Пригласить в комнату» — одно и то же действие: если ты уже в
//! комнате, приглашение зовёт собеседника в неё, если нет — открывает новую.
//!
//! Кнопка не затаскивает друга в комнату молча: приглашённый получает
//! `CallInvite`, слышит сигнал и решает сам. Пока он не ответил, он заблокирован
//! от добавления в комнату — иначе авто-синхронизация mesh (`IrohSync` раз в
//! секунду) добавляла бы его в участники в обход согласия. Блокировка адресная:
//! остальные участники группы ею не задеваются.
//!
//! Как новый участник узнаёт остальных: пригласивший в ответ на `CallAccept`
//! отправляет `RoomMembers` — список NodeID всех, кто уже в комнате. Без этого
//! список не собрался бы: новый соединён только с пригласившим, а остальные о
//! нём не знают. Дальше mesh достраивается обычным `IrohSync`.

use crate::network::message::serialize_msg;
use crate::network::p2p_iroh::IrohP2pManager;
use crate::network::NetworkMessage;
use crate::room::RoomManager;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Сколько ждём ответа на приглашение
const OUTGOING_TIMEOUT: Duration = Duration::from_secs(35);
/// Сколько звоним приглашённому, прежде чем считать приглашение пропущенным
const INCOMING_TIMEOUT: Duration = Duration::from_secs(45);
/// Датаграммы теряются: приглашение повторяем, пока не ответят
const INVITE_RESEND: Duration = Duration::from_millis(1500);
/// После отказа держим пригласившего вне комнаты. Долго, а не «пока рвётся
/// соединение»: иначе через 45 с его `IrohSync` снова затаскивал нас в комнату.
/// Снимается явно: новым приглашением или нажатием «Войти».
const BLOCK_AFTER: Duration = Duration::from_secs(3600);
/// Сколько отвечаем отказом на повторные Invite после собственного отказа
const DECLINED_AFTER: Duration = Duration::from_secs(45);
/// Причина в CallDecline
const REASON_DECLINED: u8 = 0;
const REASON_BUSY: u8 = 1;
const REASON_TIMEOUT: u8 = 2;

/// Приглашённый в комнату, который ещё не ответил.
struct Pending {
    nick: String,
    since: Instant,
    last_invite: Instant,
}

impl Pending {
    fn new(nick: String) -> Self {
        let now = Instant::now();
        Self {
            nick,
            since: now,
            last_invite: now,
        }
    }
}

enum CallPhase {
    Idle,
    /// Мы в комнате. `room_id` — идентификатор сессии: по нему отличаем
    /// приглашения этой комнаты от чужой и от запоздавших.
    Active {
        room_id: u32,
        started: Instant,
        /// Кому отправлено приглашение и ответа пока нет.
        pending: HashMap<String, Pending>,
    },
    /// Нас пригласили в чужую комнату, ответа ещё нет.
    Incoming {
        from: String,
        nick: String,
        room_id: u32,
        started: Instant,
    },
}

/// Состояние для оверлея — без сети и блокировок
#[derive(Clone, Debug)]
pub enum CallView {
    Idle,
    /// Нас пригласили
    Incoming { nick: String, elapsed: Duration },
    /// Ждём ответа от приглашённого: в комнате пока только мы
    Outgoing { nick: String, elapsed: Duration },
    /// Мы в комнате: `members` — сколько человек в ней, включая нас;
    /// `peer` — NodeID второго участника, если он ровно один (для подписи).
    Active {
        members: usize,
        peer: Option<String>,
        elapsed: Duration,
    },
}

pub struct CallManager {
    phase: Mutex<CallPhase>,
    /// NodeID → до какого момента не пускать в комнату
    blocked: Mutex<HashMap<String, Instant>>,
    /// NodeID → до какого момента отвечать отказом на повторные приглашения
    declined: Mutex<HashMap<String, Instant>>,
    notice: Mutex<Option<String>>,
    my_nick: Mutex<String>,
    my_id: u64,
    p2p: Option<Arc<IrohP2pManager>>,
    room: Arc<RoomManager>,
    is_host: Arc<AtomicBool>,
}

impl CallManager {
    pub fn new(
        my_id: u64,
        my_nick: String,
        p2p: Option<Arc<IrohP2pManager>>,
        room: Arc<RoomManager>,
        is_host: Arc<AtomicBool>,
    ) -> Self {
        Self {
            phase: Mutex::new(CallPhase::Idle),
            blocked: Mutex::new(HashMap::new()),
            declined: Mutex::new(HashMap::new()),
            notice: Mutex::new(None),
            my_nick: Mutex::new(my_nick),
            my_id,
            p2p,
            room,
            is_host,
        }
    }

    /// Ник можно поменять в настройках — приглашение должно нести актуальный
    pub fn set_my_nick(&self, nick: &str) {
        if let Ok(mut n) = self.my_nick.lock() {
            if n.as_str() != nick {
                *n = nick.to_string();
            }
        }
    }

    /// Приглашённый (или пригласивший, пока мы не ответили) не должен попасть
    /// в комнату до согласия. Остальные участники группы этим не задеваются:
    /// блокировка адресная, по NodeID.
    pub fn is_blocked(&self, peer: &str) -> bool {
        if let Ok(phase) = self.phase.lock() {
            match &*phase {
                CallPhase::Incoming { from, .. } if from == peer => return true,
                CallPhase::Active { pending, .. } if pending.contains_key(peer) => return true,
                _ => {}
            }
        }
        self.blocked_now(peer)
    }

    /// Только карта блокировок — можно звать, держа замок фазы
    fn blocked_now(&self, peer: &str) -> bool {
        let now = Instant::now();
        self.blocked
            .lock()
            .map(|m| m.get(peer).map(|until| now < *until).unwrap_or(false))
            .unwrap_or(false)
    }

    /// Мы сами отказали этому узлу недавно — его повторные Invite отклоняем
    fn declined_now(&self, peer: &str) -> bool {
        let now = Instant::now();
        self.declined
            .lock()
            .map(|m| m.get(peer).map(|until| now < *until).unwrap_or(false))
            .unwrap_or(false)
    }

    pub fn view(&self) -> CallView {
        let Ok(phase) = self.phase.lock() else {
            return CallView::Idle;
        };
        let now = Instant::now();
        match &*phase {
            CallPhase::Idle => CallView::Idle,
            CallPhase::Incoming { nick, started, .. } => CallView::Incoming {
                nick: nick.clone(),
                elapsed: now.duration_since(*started),
            },
            CallPhase::Active {
                pending, started, ..
            } => {
                let peers = self.room.get_all_peers();
                // Пока в комнате никого нет — это «звоним…». Как только кто-то
                // вошёл, приглашение следующего не должно превращать разговор
                // обратно в карточку звонка.
                if peers.is_empty() {
                    if let Some(p) = pending.values().max_by_key(|p| p.since) {
                        return CallView::Outgoing {
                            nick: p.nick.clone(),
                            elapsed: now.duration_since(p.since),
                        };
                    }
                }
                CallView::Active {
                    members: peers.len() + 1,
                    peer: if peers.len() == 1 {
                        peers.first().map(|p| p.address.clone())
                    } else {
                        None
                    },
                    elapsed: now.duration_since(*started),
                }
            }
        }
    }

    /// 0 — тихо, 1 — входящее приглашение, 2 — гудок «звоним…».
    /// В комнате на N гудка нет: приглашение ещё одного человека не должно
    /// заливать эфир сигналом.
    pub fn ring_mode(&self) -> u32 {
        match self.view() {
            CallView::Incoming { .. } => 1,
            CallView::Outgoing { .. } => 2,
            _ => 0,
        }
    }

    pub fn take_notice(&self) -> Option<String> {
        self.notice.lock().ok().and_then(|mut n| n.take())
    }

    fn set_notice(&self, text: String) {
        if let Ok(mut n) = self.notice.lock() {
            *n = Some(text);
        }
    }

    /// Пригласить в комнату. Из Idle открывает новую комнату, из Active зовёт
    /// в текущую; повторный вызов для уже приглашённого безопасен — повтор
    /// отправит `tick`.
    pub fn start_outgoing(&self, peer: &str, peer_nick: &str, my_nick: &str) -> bool {
        if let Some(known) = self.room.get_all_peers().into_iter().find(|p| p.address == peer) {
            // Запись могла остаться после «Выйти из эфира» собеседника: он
            // больше не шлёт heartbeat, но список обновляется по таймауту не
            // мгновенно. Если он давно молчит — принимаем приглашение заново,
            // иначе честно говорим, что он уже здесь.
            // last_heartbeat — миллисекунды с эпохи, как и в migration.rs.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let silent = now.saturating_sub(known.last_heartbeat) >= 6000;
            if silent {
                self.room.remove_peer(known.id);
            } else {
                self.set_notice("Он уже в комнате".to_string());
                return false;
            }
        }

        enum Act {
            Invite { room_id: u32 },
            /// Одновременное приглашение друг другу: принимаем встречное
            AcceptMutual { room_id: u32 },
            None,
        }

        let act = {
            let Ok(mut phase) = self.phase.lock() else {
                return false;
            };
            match &mut *phase {
                CallPhase::Idle => {
                    let room_id = new_room_id(self.my_id);
                    let mut pending = HashMap::new();
                    pending.insert(peer.to_string(), Pending::new(peer_nick.to_string()));
                    *phase = CallPhase::Active {
                        room_id,
                        started: Instant::now(),
                        pending,
                    };
                    Act::Invite { room_id }
                }
                CallPhase::Active {
                    room_id, pending, ..
                } => {
                    let id = *room_id;
                    if !pending.contains_key(peer) {
                        pending.insert(peer.to_string(), Pending::new(peer_nick.to_string()));
                    }
                    Act::Invite { room_id: id }
                }
                CallPhase::Incoming {
                    from, room_id, ..
                } if from == peer => {
                    let id = *room_id;
                    *phase = CallPhase::Active {
                        room_id: id,
                        started: Instant::now(),
                        pending: HashMap::new(),
                    };
                    Act::AcceptMutual { room_id: id }
                }
                CallPhase::Incoming { .. } => {
                    self.set_notice("Сначала ответьте на приглашение".to_string());
                    Act::None
                }
            }
        };

        match act {
            Act::Invite { room_id } => {
                // Приглашение — явное возвращение в эфир
                self.room.join();
                self.unblock(peer);
                self.send_invite(peer, room_id, my_nick);
                true
            }
            Act::AcceptMutual { room_id } => {
                self.room.join();
                self.unblock(peer);
                self.send_accept_repeat(peer, room_id);
                println!(
                    "[КОМНАТА] Встречное приглашение с {} — входим",
                    short(peer)
                );
                true
            }
            Act::None => false,
        }
    }

    /// Войти по приглашению
    pub fn accept_incoming(&self) -> bool {
        let joined = {
            let Ok(mut phase) = self.phase.lock() else {
                return false;
            };
            if let CallPhase::Incoming { from, room_id, .. } = &*phase {
                let from = from.clone();
                let room_id = *room_id;
                *phase = CallPhase::Active {
                    room_id,
                    started: Instant::now(),
                    pending: HashMap::new(),
                };
                Some((from, room_id))
            } else {
                None
            }
        };
        let Some((from, room_id)) = joined else {
            return false;
        };
        // Согласие получено — входим в эфир, снимаем блок и подтверждаем приём.
        // Мы гости: хост — пригласивший (выборы по меньшему id всё равно
        // переиграют это внутри комнаты).
        println!("[КОМНАТА] Входим по приглашению {} (комната {})", short(&from), room_id);
        self.room.join();
        // Пригласивший сразу становится соседом: иначе мы «в комнате», но без
        // участников, и mesh не может до него дотянуться по списку.
        self.register_peer(&from);
        self.is_host.store(false, Ordering::SeqCst);
        self.unblock(&from);
        self.send_accept_repeat(&from, room_id);
        true
    }

    pub fn decline_incoming(&self) -> bool {
        let declined = {
            let Ok(mut phase) = self.phase.lock() else {
                return false;
            };
            if let CallPhase::Incoming { from, room_id, .. } = &*phase {
                let call = (from.clone(), *room_id);
                *phase = CallPhase::Idle;
                Some(call)
            } else {
                None
            }
        };
        let Some((from, room_id)) = declined else {
            return false;
        };
        self.send_decline(&from, room_id, REASON_DECLINED);
        self.mark_declined(&from);
        self.block(&from, BLOCK_AFTER);
        self.disconnect_later(&from, 800);
        true
    }

    /// Отменить исходящие приглашения. Если в комнате уже кто-то есть, комната
    /// остаётся: отменяется приглашение, а не разговор.
    pub fn cancel_outgoing(&self) {
        let (cancelled, leave) = {
            let Ok(mut phase) = self.phase.lock() else {
                return;
            };
            match &mut *phase {
                CallPhase::Active {
                    room_id, pending, ..
                } => {
                    let id = *room_id;
                    let list: Vec<String> = pending.keys().cloned().collect();
                    pending.clear();
                    let empty = self.room.get_all_peers().is_empty();
                    if empty {
                        *phase = CallPhase::Idle;
                    }
                    (list.into_iter().map(|p| (p, id)).collect::<Vec<_>>(), empty)
                }
                _ => (Vec::new(), false),
            }
        };

        for (peer, room_id) in cancelled {
            self.send_to(
                &peer,
                NetworkMessage::CallCancel {
                    sender_id: self.my_id,
                    room_id,
                },
            );
            self.drop_from_room(&peer);
            self.disconnect_later(&peer, 600);
        }
        if leave {
            // Никого не осталось — как «выйти из эфира»: иначе авто-синхронизация
            // вернёт участников в комнату за секунду.
            self.room.leave();
        }
    }

    /// Выйти из комнаты (кнопка «Завершить» и Esc).
    ///
    /// Остальные участники остаются друг у друга: уходит только нажавший.
    /// Соединения рвём, а `room.leave()` не даёт `IrohSync` вернуть нас назад.
    pub fn end_active(&self) {
        let (peers, room_id) = {
            let Ok(mut phase) = self.phase.lock() else {
                return;
            };
            match &mut *phase {
                CallPhase::Active {
                    room_id, pending, ..
                } => {
                    let id = *room_id;
                    let mut peers: Vec<String> = self
                        .room
                        .get_all_peers()
                        .into_iter()
                        .map(|p| p.address)
                        .collect();
                    peers.extend(pending.keys().cloned());
                    *phase = CallPhase::Idle;
                    (peers, id)
                }
                CallPhase::Incoming { from, room_id, .. } => {
                    let peers = vec![from.clone()];
                    let id = *room_id;
                    *phase = CallPhase::Idle;
                    (peers, id)
                }
                CallPhase::Idle => (Vec::new(), 0),
            }
        };

        for peer in &peers {
            self.send_to(
                peer,
                NetworkMessage::CallEnd {
                    sender_id: self.my_id,
                    room_id,
                },
            );
        }
        self.room.leave();
        if peers.is_empty() {
            return;
        }
        let Some(p2p) = self.p2p.clone() else {
            return;
        };
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        rt.spawn(async move {
            tokio::time::sleep(Duration::from_millis(400)).await;
            p2p.disconnect_all().await;
        });
    }

    // ————— Входящие сообщения (из iroh_service) —————

    pub fn on_invite(&self, peer: &str, nick: &str, room_id: u32) {
        enum Act {
            None,
            Decline(u8),
            /// Приглашение в комнату, в которой мы уже находимся — подтверждаем
            Accept(u32),
            /// Встречные приглашения: оба позвонили одновременно. Переходим в
            /// комнату собеседника и отменяем свои приглашения — они уже с
            /// другим номером комнаты и никем не будут приняты.
            Switch {
                room_id: u32,
                /// Прежний номер комнаты: по нему отменяем свои приглашения
                old_room_id: u32,
                cancelled: Vec<String>,
            },
        }
        let act = {
            let Ok(mut phase) = self.phase.lock() else {
                return;
            };
            match &mut *phase {
                CallPhase::Idle => {
                    if self.declined_now(peer) {
                        Act::Decline(REASON_DECLINED)
                    } else {
                        *phase = CallPhase::Incoming {
                            from: peer.to_string(),
                            nick: nick.to_string(),
                            room_id,
                            started: Instant::now(),
                        };
                        Act::None
                    }
                }
                CallPhase::Incoming {
                    from,
                    nick: n,
                    room_id: r,
                    started,
                } => {
                    if from == peer {
                        // Повтор нашего же приглашения: звеним дальше, но новый
                        // room_id означает, что собеседник открыл комнату заново
                        if *r != room_id {
                            *n = nick.to_string();
                            *r = room_id;
                            *started = Instant::now();
                        }
                        Act::None
                    } else {
                        Act::Decline(REASON_BUSY)
                    }
                }
                CallPhase::Active {
                    room_id: r,
                    pending,
                    ..
                } => {
                    if *r == room_id {
                        // Мы уже в этой комнате: отвечаем согласием, чтобы
                        // пригласивший перестал звонить
                        Act::Accept(room_id)
                    } else if pending.contains_key(peer) && self.room.get_all_peers().is_empty() {
                        let cancelled: Vec<String> =
                            pending.keys().filter(|p| *p != peer).cloned().collect();
                        pending.clear();
                        let old = *r;
                        *r = room_id;
                        Act::Switch {
                            room_id,
                            old_room_id: old,
                            cancelled,
                        }
                    } else {
                        Act::Decline(REASON_BUSY)
                    }
                }
            }
        };
        match act {
            Act::None => {}
            Act::Decline(reason) => self.send_decline(peer, room_id, reason),
            Act::Accept(id) => {
                println!(
                    "[КОМНАТА] Приглашение в свою же комнату от {} — подтверждаем",
                    short(peer)
                );
                self.room.join();
                self.unblock(peer);
                self.send_accept_repeat(peer, id);
            }
            Act::Switch {
                room_id: id,
                old_room_id,
                cancelled,
            } => {
                println!(
                    "[КОМНАТА] Встречные приглашения с {} — переходим в его комнату",
                    short(peer)
                );
                for other in cancelled {
                    self.send_to(
                        &other,
                        NetworkMessage::CallCancel {
                            sender_id: self.my_id,
                            room_id: old_room_id,
                        },
                    );
                    self.disconnect_later(&other, 600);
                }
                self.room.join();
                self.unblock(peer);
                self.send_accept_repeat(peer, id);
            }
        }
    }

    /// `room_id` должен совпасть: иначе запоздавший Accept от прошлой комнаты
    /// поднимал бы новую (и оверлей «В звонке» без собеседника).
    pub fn on_accept(&self, peer: &str, room_id: u32) {
        let joined = {
            let Ok(mut phase) = self.phase.lock() else {
                return;
            };
            match &mut *phase {
                CallPhase::Active {
                    room_id: r,
                    pending,
                    ..
                } if *r == room_id => {
                    pending.remove(peer);
                    true
                }
                CallPhase::Incoming {
                    from,
                    room_id: r,
                    ..
                } if from == peer && *r == room_id => {
                    *phase = CallPhase::Active {
                        room_id,
                        started: Instant::now(),
                        pending: HashMap::new(),
                    };
                    true
                }
                _ => false,
            }
        };
        if !joined {
            return;
        }
        self.room.join();
        self.unblock(peer);
        // Вошедший — участник комнаты с этой секунды. Без записи его нет в
        // списке, mesh не дотягивается до него (ensure_connected идёт по
        // списку), и человек остаётся «приглашённым», но невидимым.
        self.register_peer(peer);
        // Новый участник соединён только с нами: отдаём ему список остальных.
        self.send_members(peer, room_id);
        println!("[КОМНАТА] {} вошёл в комнату", short(peer));
    }

    pub fn on_decline(&self, peer: &str, room_id: u32, reason: u8) {
        let mut handled = false;
        let mut close_room = false;
        {
            let Ok(mut phase) = self.phase.lock() else {
                return;
            };
            match &mut *phase {
                CallPhase::Active {
                    room_id: r,
                    pending,
                    ..
                } if *r == room_id => {
                    handled = pending.remove(peer).is_some() || handled;
                    let others = self
                        .room
                        .get_all_peers()
                        .iter()
                        .filter(|p| p.address != peer)
                        .count();
                    if others == 0 && pending.is_empty() {
                        *phase = CallPhase::Idle;
                        close_room = true;
                    }
                }
                CallPhase::Incoming { from, room_id: r, .. }
                    if from == peer && *r == room_id =>
                {
                    *phase = CallPhase::Idle;
                    handled = true;
                    close_room = true;
                }
                _ => {}
            }
        }
        let _ = room_id;
        if !handled {
            return;
        }
        if reason == REASON_BUSY {
            self.set_notice("Собеседник занят".to_string());
        } else {
            self.set_notice("Приглашение отклонено".to_string());
        }
        self.block(peer, BLOCK_AFTER);
        self.drop_from_room(peer);
        self.disconnect_later(peer, 400);
        if close_room {
            self.room.leave();
        }
    }

    pub fn on_cancel(&self, peer: &str, room_id: u32) {
        let nick = {
            let Ok(mut phase) = self.phase.lock() else {
                return;
            };
            match &mut *phase {
                CallPhase::Incoming {
                    from,
                    nick,
                    room_id: r,
                    ..
                } if from == peer && *r == room_id => {
                    let nick = nick.clone();
                    *phase = CallPhase::Idle;
                    Some(nick)
                }
                _ => None,
            }
        };
        if let Some(nick) = nick {
            self.set_notice(format!("Пропущенное приглашение: {nick}"));
            self.block(peer, BLOCK_AFTER);
            self.disconnect_later(peer, 400);
        }
    }

    /// Участник вышел из комнаты. Остальные продолжают разговор; комната
    /// закрывается только когда в ней никого не осталось.
    pub fn on_end(&self, peer: &str, room_id: u32) {
        let mut handled = false;
        let mut close_room = false;
        {
            let Ok(mut phase) = self.phase.lock() else {
                return;
            };
            match &mut *phase {
                CallPhase::Active {
                    room_id: r,
                    pending,
                    ..
                } if *r == room_id => {
                    pending.remove(peer);
                    handled = true;
                    let others = self
                        .room
                        .get_all_peers()
                        .iter()
                        .filter(|p| p.address != peer)
                        .count();
                    if others == 0 && pending.is_empty() {
                        *phase = CallPhase::Idle;
                        close_room = true;
                    }
                }
                CallPhase::Incoming { from, room_id: r, .. }
                    if from == peer && *r == room_id =>
                {
                    *phase = CallPhase::Idle;
                    handled = true;
                    close_room = true;
                }
                _ => {}
            }
        }
        if !handled {
            return;
        }
        self.drop_from_room(peer);
        if close_room {
            self.room.leave();
        }
        self.set_notice("Собеседник вышел из комнаты".to_string());
    }

    /// Раз в кадр: повтор приглашений и таймауты
    pub fn tick(&self) {
        let now = Instant::now();
        let mut resend: Vec<(String, u32)> = Vec::new();
        let mut expired: Vec<(String, String)> = Vec::new();
        let mut incoming_timeout: Option<(String, String, u32)> = None;
        let mut close_room = false;
        {
            let Ok(mut phase) = self.phase.lock() else {
                return;
            };
            match &mut *phase {
                CallPhase::Active {
                    room_id, pending, ..
                } => {
                    let id = *room_id;
                    let stale: Vec<String> = pending
                        .iter()
                        .filter(|(_, p)| now.duration_since(p.since) >= OUTGOING_TIMEOUT)
                        .map(|(peer, _)| peer.clone())
                        .collect();
                    for peer in stale {
                        if let Some(p) = pending.remove(&peer) {
                            expired.push((peer, p.nick));
                        }
                    }
                    for (peer, p) in pending.iter_mut() {
                        if now.duration_since(p.last_invite) >= INVITE_RESEND {
                            p.last_invite = now;
                            resend.push((peer.clone(), id));
                        }
                    }
                    if pending.is_empty() && self.room.get_all_peers().is_empty() {
                        // Приглашали, никто не пришёл, в комнате мы одни
                        *phase = CallPhase::Idle;
                        close_room = true;
                    }
                }
                CallPhase::Incoming {
                    from,
                    nick,
                    room_id,
                    started,
                } => {
                    if now.duration_since(*started) >= INCOMING_TIMEOUT {
                        incoming_timeout = Some((from.clone(), nick.clone(), *room_id));
                        *phase = CallPhase::Idle;
                    }
                }
                CallPhase::Idle => {}
            }
        }

        if close_room {
            self.room.leave();
        }

        for (peer, room_id) in resend {
            let my_nick = self.my_nick.lock().map(|n| n.clone()).unwrap_or_default();
            self.send_invite(&peer, room_id, &my_nick);
        }

        for (peer, nick) in expired {
            self.set_notice(format!("{nick} не ответил"));
            self.block(&peer, BLOCK_AFTER);
            self.drop_from_room(&peer);
            self.disconnect_later(&peer, 400);
        }

        if let Some((peer, nick, room_id)) = incoming_timeout {
            self.set_notice(format!("Пропущенное приглашение: {nick}"));
            self.send_decline(&peer, room_id, REASON_TIMEOUT);
            self.mark_declined(&peer);
            self.block(&peer, BLOCK_AFTER);
            self.disconnect_later(&peer, 400);
        }
    }

    // ————— Внутреннее —————

    fn send_invite(&self, peer: &str, room_id: u32, my_nick: &str) {
        self.send_to(
            peer,
            NetworkMessage::CallInvite {
                sender_id: self.my_id,
                room_id,
                nick: my_nick.to_string(),
            },
        );
    }

    fn send_to(&self, peer: &str, msg: NetworkMessage) {
        let Some(p2p) = self.p2p.clone() else {
            eprintln!("[КОМНАТА] Сеть не запущена — сообщение не отправлено");
            return;
        };
        let peer = peer.to_string();
        let bytes = serialize_msg(&msg);
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            eprintln!("[КОМНАТА] Нет tokio-рантайма для отправки");
            return;
        };
        rt.spawn(async move {
            let connected = p2p.get_connected_peers().await;
            if !connected.iter().any(|p| p == &peer) {
                if let Err(e) = p2p.connect_to_peer(&peer).await {
                    eprintln!(
                        "[КОМНАТА] Не удалось подключиться к {}: {}",
                        short(&peer),
                        e
                    );
                    return;
                }
            }
            if let Err(e) = p2p.send_bytes(&peer, &bytes).await {
                eprintln!("[КОМНАТА] Ошибка отправки {}: {}", short(&peer), e);
            }
        });
    }

    fn send_decline(&self, peer: &str, room_id: u32, reason: u8) {
        self.send_to(
            peer,
            NetworkMessage::CallDecline {
                sender_id: self.my_id,
                room_id,
                reason,
            },
        );
        if reason == REASON_DECLINED {
            self.block(peer, BLOCK_AFTER);
        }
    }

    /// Accept шлём трижды: один потерянный датаграм не должен оставить
    /// пригласившего звонить уже после ответа.
    fn send_accept_repeat(&self, peer: &str, room_id: u32) {
        let Some(p2p) = self.p2p.clone() else {
            return;
        };
        let peer = peer.to_string();
        let bytes = serialize_msg(&NetworkMessage::CallAccept {
            sender_id: self.my_id,
            room_id,
        });
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        rt.spawn(async move {
            let connected = p2p.get_connected_peers().await;
            if !connected.iter().any(|p| p == &peer) {
                let _ = p2p.connect_to_peer(&peer).await;
            }
            for i in 0..3 {
                let _ = p2p.send_bytes(&peer, &bytes).await;
                if i < 2 {
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
            }
        });
    }

    /// Список комнаты новому участнику: без него он соединён только с нами.
    fn send_members(&self, peer: &str, room_id: u32) {
        let members: Vec<String> = self
            .room
            .get_all_peers()
            .into_iter()
            .map(|p| p.address)
            .filter(|address| address != peer)
            .collect();
        self.send_to(
            peer,
            NetworkMessage::RoomMembers {
                sender_id: self.my_id,
                room_id,
                members,
            },
        );
    }

    fn drop_from_room(&self, peer: &str) {
        if let Some(id) = crate::identity::my_id_from_node_id(peer) {
            self.room.remove_peer(id);
        }
    }

    /// Даём последнему сообщению уйти и рвём соединение: иначе IrohSync
    /// раз в секунду снова добавил бы собеседника в комнату.
    fn disconnect_later(&self, peer: &str, delay_ms: u64) {
        let Some(p2p) = self.p2p.clone() else {
            return;
        };
        let peer = peer.to_string();
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            return;
        };
        rt.spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            p2p.disconnect_peer(&peer).await;
        });
    }

    fn block(&self, peer: &str, dur: Duration) {
        if let Ok(mut m) = self.blocked.lock() {
            let now = Instant::now();
            m.retain(|_, until| now < *until);
            m.insert(peer.to_string(), now + dur);
        }
    }

    /// Запомнили собственный отказ: повторы Invite от этого узла отклоняем
    fn mark_declined(&self, peer: &str) {
        if let Ok(mut m) = self.declined.lock() {
            let now = Instant::now();
            m.retain(|_, until| now < *until);
            m.insert(peer.to_string(), now + DECLINED_AFTER);
        }
    }

    fn unblock(&self, peer: &str) {
        if let Ok(mut m) = self.blocked.lock() {
            m.remove(peer);
        }
        if let Ok(mut m) = self.declined.lock() {
            m.remove(peer);
        }
    }

    /// Явный вход в комнату по NodeID снимает все ограничения с этого узла.
    /// «Выйти из эфира», когда звонка нет: комната собрана mesh-синхронизацией,
    /// и о нас знают без всякого `CallPhase::Active`. Явно объявляем уход, иначе
    /// остальные держат нас «нет связи» до таймаута (12 с), не могут пригласить
    /// обратно и видят устаревший список участников.
    /// Добавляет соседа в комнату по NodeID (heartbeat — сейчас).
    ///
    /// Нужно в момент входа: IrohSync придёт только после того, как мы сами
    /// начнём рассылать свой, а рассылка и лечение соединений идут по списку
    /// участников. Без записи вошедший не видит никого, хотя «в комнате».
    fn register_peer(&self, node_id: &str) {
        let Some(id) = crate::identity::my_id_from_node_id(node_id) else {
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.room.update_peer(crate::types::Peer {
            id,
            address: node_id.to_string(),
            is_host: false,
            last_heartbeat: now,
        });
    }

    pub fn announce_leave(&self) {
        let members = self.room.get_all_addresses();
        if members.is_empty() {
            return;
        }
        let room_id = self.room_id().unwrap_or(0);
        for member in members {
            self.send_to(
                &member,
                crate::network::message::NetworkMessage::CallEnd {
                    sender_id: self.my_id,
                    room_id,
                },
            );
        }
    }

    pub fn unblock_peer(&self, peer: &str) {
        self.unblock(peer);
    }

    /// Номер комнаты, в которой мы сейчас: по нему проверяется `RoomMembers`.
    /// Его знают только те, кого пригласили, поэтому чужой список комнаты
    /// подсунуть не может.
    pub fn room_id(&self) -> Option<u32> {
        match &*self.phase.lock().ok()? {
            CallPhase::Active { room_id, .. } => Some(*room_id),
            _ => None,
        }
    }
}

fn new_room_id(my_id: u64) -> u32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    // Номер комнаты должен отличаться у двух узлов, открывших комнаты в одну
    // и ту же микросекунду: мешаем id узла.
    ((my_id as u32).wrapping_mul(2_654_435_761) ^ nanos) | 1
}

fn short(node_id: &str) -> &str {
    &node_id[..node_id.len().min(10)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Peer;

    /// Валидный 64-символьный NodeID для тестов (hex): по нему считается id.
    fn node(tag: u8) -> String {
        format!("{tag:02x}").repeat(32)
    }

    fn manager() -> CallManager {
        CallManager::new(
            1,
            "Я".to_string(),
            None,
            Arc::new(RoomManager::new(1)),
            Arc::new(AtomicBool::new(true)),
        )
    }

    /// Кладём участника в комнату так, как это делает IrohSync.
    fn now_ms() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Сосед в комнате, от которого только что был heartbeat.
    fn add_to_room(m: &CallManager, node_id: &str) {
        add_to_room_at(m, node_id, now_ms());
    }

    fn add_to_room_at(m: &CallManager, node_id: &str, heartbeat: u64) {
        let id = crate::identity::my_id_from_node_id(node_id).expect("валидный NodeID");
        m.room.update_peer(Peer {
            id,
            address: node_id.to_string(),
            is_host: false,
            last_heartbeat: heartbeat,
        });
    }

    fn room_id_of(m: &CallManager) -> u32 {
        match &*m.phase.lock().unwrap() {
            CallPhase::Active { room_id, .. } | CallPhase::Incoming { room_id, .. } => *room_id,
            CallPhase::Idle => panic!("фаза Idle"),
        }
    }

    #[test]
    fn invite_then_accept_opens_room() {
        let a = node(0xAA);
        let m = manager();
        assert!(m.start_outgoing(&a, "Аня", "Я"));
        assert!(matches!(m.view(), CallView::Outgoing { .. }));
        assert!(m.is_blocked(&a), "до согласия в комнату нельзя");

        let id = room_id_of(&m);
        m.on_accept(&a, id);
        add_to_room(&m, &a);
        assert!(!m.is_blocked(&a));
        match m.view() {
            CallView::Active { members, peer, .. } => {
                assert_eq!(members, 2);
                assert_eq!(peer.as_deref(), Some(a.as_str()));
            }
            other => panic!("ожидали Active, получили {other:?}"),
        }
    }

    #[test]
    fn accept_with_wrong_room_id_ignored() {
        let a = node(0xAA);
        let m = manager();
        m.start_outgoing(&a, "Аня", "Я");
        let id = room_id_of(&m);
        m.on_accept(&a, id ^ u32::MAX);
        assert!(matches!(m.view(), CallView::Outgoing { .. }));
        m.on_accept(&a, id);
        add_to_room(&m, &a);
        assert!(matches!(m.view(), CallView::Active { .. }));
    }

    #[test]
    fn second_person_can_be_invited_into_existing_room() {
        let (a, b) = (node(0xAA), node(0xBB));
        let m = manager();
        m.start_outgoing(&a, "Аня", "Я");
        let id = room_id_of(&m);
        m.on_accept(&a, id);
        add_to_room(&m, &a);

        // Группа: зовём второго в ту же комнату, а не «вы уже в звонке»
        assert!(m.start_outgoing(&b, "Боря", "Я"));
        assert_eq!(room_id_of(&m), id);
        // В комнате уже есть Аня — карточка «звоним» не появляется
        assert!(matches!(m.view(), CallView::Active { .. }));
        assert!(m.is_blocked(&b));

        m.on_accept(&b, id);
        add_to_room(&m, &b);
        match m.view() {
            CallView::Active { members, peer, .. } => {
                assert_eq!(members, 3);
                assert!(peer.is_none(), "участников больше одного — подписи нет");
            }
            other => panic!("ожидали Active, получили {other:?}"),
        }
    }

    #[test]
    fn inviting_someone_already_in_room_is_refused() {
        let a = node(0xAA);
        let m = manager();
        m.start_outgoing(&a, "Аня", "Я");
        add_to_room(&m, &a);
        assert!(!m.start_outgoing(&a, "Аня", "Я"));
        assert_eq!(m.take_notice().as_deref(), Some("Он уже в комнате"));
    }

    #[test]
    fn stale_room_entry_does_not_block_invite() {
        // Собеседник нажал «Выйти из эфира», а запись о нём ещё висит в списке:
        // приглашение должно уйти, а не «он уже в комнате».
        let a = node(0xAA);
        let m = manager();
        add_to_room_at(&m, &a, now_ms().saturating_sub(30_000));
        assert!(m.start_outgoing(&a, "Аня", "Я"), "приглашение уходит");
        assert_eq!(m.room.get_all_peers().len(), 0, "мёртвая запись убрана");
    }

    #[test]
    fn incoming_accept_and_leave() {
        let b = node(0xBB);
        let m = manager();
        m.on_invite(&b, "Боря", 77);
        assert!(matches!(m.view(), CallView::Incoming { .. }));
        assert!(m.is_blocked(&b));
        assert!(m.accept_incoming());
        add_to_room(&m, &b);
        assert!(matches!(m.view(), CallView::Active { .. }));
        assert!(!m.is_blocked(&b));

        m.end_active();
        assert!(matches!(m.view(), CallView::Idle));
        assert!(m.room.is_left(), "после выхода автосинхронизация не вернёт");
    }

    #[test]
    fn decline_blocks_and_closes_empty_room() {
        let a = node(0xAA);
        let m = manager();
        m.start_outgoing(&a, "Аня", "Я");
        let id = room_id_of(&m);
        m.on_decline(&a, id, REASON_DECLINED);
        assert!(matches!(m.view(), CallView::Idle));
        assert!(m.is_blocked(&a));
        assert_eq!(m.take_notice().as_deref(), Some("Приглашение отклонено"));
    }

    #[test]
    fn decline_while_others_are_in_room_keeps_call() {
        let (a, b) = (node(0xAA), node(0xBB));
        let m = manager();
        m.start_outgoing(&a, "Аня", "Я");
        let id = room_id_of(&m);
        m.on_accept(&a, id);
        add_to_room(&m, &a);
        m.start_outgoing(&b, "Боря", "Я");
        m.on_decline(&b, id, REASON_DECLINED);
        assert!(
            matches!(m.view(), CallView::Active { members, .. } if members == 2),
            "отказ одного не закрывает комнату для остальных"
        );
        assert!(!m.room.is_left());
    }

    #[test]
    fn busy_reason_notice() {
        let a = node(0xAA);
        let m = manager();
        m.start_outgoing(&a, "Аня", "Я");
        let id = room_id_of(&m);
        m.on_decline(&a, id, REASON_BUSY);
        assert_eq!(m.take_notice().as_deref(), Some("Собеседник занят"));
    }

    #[test]
    fn cancel_keeps_room_when_someone_joined() {
        let (a, b) = (node(0xAA), node(0xBB));
        let m = manager();
        m.start_outgoing(&a, "Аня", "Я");
        let id = room_id_of(&m);
        m.on_accept(&a, id);
        add_to_room(&m, &a);
        m.start_outgoing(&b, "Боря", "Я");
        m.cancel_outgoing();
        assert!(matches!(m.view(), CallView::Active { .. }));
        assert!(!m.room.is_left());
    }

    #[test]
    fn cancel_closes_room_when_nobody_came() {
        let a = node(0xAA);
        let m = manager();
        m.start_outgoing(&a, "Аня", "Я");
        m.cancel_outgoing();
        assert!(matches!(m.view(), CallView::Idle));
        assert!(m.room.is_left());
    }

    #[test]
    fn cancel_with_wrong_room_id_ignored() {
        let b = node(0xBB);
        let m = manager();
        m.on_invite(&b, "Боря", 5);
        m.on_cancel(&b, 6);
        assert!(matches!(m.view(), CallView::Incoming { .. }));
        m.on_cancel(&b, 5);
        assert!(matches!(m.view(), CallView::Idle));
    }

    #[test]
    fn second_invite_gets_busy() {
        let (b, c) = (node(0xBB), node(0xCC));
        let m = manager();
        m.on_invite(&b, "Боря", 1);
        m.on_invite(&c, "Вова", 2);
        match m.view() {
            CallView::Incoming { nick, .. } => assert_eq!(nick, "Боря"),
            other => panic!("ожидали Incoming, получили {other:?}"),
        }
    }

    #[test]
    fn mutual_invitation_becomes_room() {
        let a = node(0xAA);
        let m = manager();
        m.start_outgoing(&a, "Аня", "Я");
        let ours = room_id_of(&m);
        assert_ne!(ours, 42);
        // Встречное приглашение в другую комнату — переходим в неё
        m.on_invite(&a, "Аня", 42);
        assert!(matches!(m.view(), CallView::Active { .. }));
        assert_eq!(room_id_of(&m), 42, "остаёмся в комнате пригласившего");
    }

    #[test]
    fn decline_repeats_after_own_refusal() {
        let b = node(0xBB);
        let m = manager();
        m.on_invite(&b, "Боря", 9);
        assert!(m.decline_incoming());
        assert!(matches!(m.view(), CallView::Idle));
        // пригласивший повторяет Invite — снова отказ, карточка не всплывает
        m.on_invite(&b, "Боря", 10);
        assert!(matches!(m.view(), CallView::Idle));
    }

    #[test]
    fn stale_end_ignored() {
        let b = node(0xBB);
        let m = manager();
        m.on_invite(&b, "Боря", 11);
        assert!(m.accept_incoming());
        add_to_room(&m, &b);
        m.on_end(&b, 12);
        assert!(matches!(m.view(), CallView::Active { .. }));
        m.on_end(&b, 11);
        assert!(matches!(m.view(), CallView::Idle));
    }

    #[test]
    fn peer_leaving_group_keeps_room_for_others() {
        let (a, b) = (node(0xAA), node(0xBB));
        let m = manager();
        m.start_outgoing(&a, "Аня", "Я");
        let id = room_id_of(&m);
        m.on_accept(&a, id);
        add_to_room(&m, &a);
        m.start_outgoing(&b, "Боря", "Я");
        m.on_accept(&b, id);
        add_to_room(&m, &b);

        m.on_end(&b, id);
        match m.view() {
            CallView::Active { members, .. } => assert_eq!(members, 2),
            other => panic!("ожидали Active с одним собеседником, получили {other:?}"),
        }
        m.on_end(&a, id);
        assert!(matches!(m.view(), CallView::Idle));
    }

    #[test]
    fn invite_to_same_room_is_confirmed() {
        let b = node(0xBB);
        let m = manager();
        m.on_invite(&b, "Боря", 21);
        assert!(m.accept_incoming());
        add_to_room(&m, &b);
        // Пришло приглашение в комнату, в которой мы уже сидим
        m.on_invite(&b, "Боря", 21);
        assert!(matches!(m.view(), CallView::Active { .. }));
        assert!(!m.is_blocked(&b));
    }

    #[test]
    fn invite_from_other_room_is_busy() {
        let (b, c) = (node(0xBB), node(0xCC));
        let m = manager();
        m.on_invite(&b, "Боря", 21);
        m.accept_incoming();
        add_to_room(&m, &b);
        m.on_invite(&c, "Вова", 22);
        assert!(matches!(m.view(), CallView::Active { .. }));
    }

    #[test]
    fn ring_only_when_alone_in_room() {
        let (a, b) = (node(0xAA), node(0xBB));
        let m = manager();
        assert_eq!(m.ring_mode(), 0);
        m.start_outgoing(&a, "Аня", "Я");
        assert_eq!(m.ring_mode(), 2, "ждём первого ответа — гудим");
        let id = room_id_of(&m);
        m.on_accept(&a, id);
        add_to_room(&m, &a);
        assert_eq!(m.ring_mode(), 0, "разговор идёт — сигнала нет");
        m.start_outgoing(&b, "Боря", "Я");
        assert_eq!(m.ring_mode(), 0, "приглашение в группу не заливает эфир");
    }

    #[test]
    fn mutual_invite_with_people_in_room_is_busy() {
        let (a, b) = (node(0xAA), node(0xBB));
        let m = manager();
        m.start_outgoing(&a, "Аня", "Я");
        let id = room_id_of(&m);
        m.on_accept(&a, id);
        add_to_room(&m, &a);

        // Мы уже разговариваем с Аней: второе приглашение с другим room_id не
        // должно смешать две комнаты — отвечаем «занято» и остаёмся в своей.
        m.on_invite(&b, "Боря", 999);
        assert_eq!(room_id_of(&m), id);
        match m.view() {
            CallView::Active { members, .. } => assert_eq!(members, 2),
            other => panic!("ожидали Active, получили {other:?}"),
        }
    }

    #[test]
    fn room_id_is_shared_for_members_list() {
        let a = node(0xAA);
        let m = manager();
        assert!(m.room_id().is_none(), "вне комнаты номера нет");
        m.start_outgoing(&a, "Аня", "Я");
        let id = room_id_of(&m);
        assert_eq!(m.room_id(), Some(id));
        m.end_active();
        assert!(m.room_id().is_none());
    }

    #[test]
    fn two_nodes_get_different_room_ids() {
        // Номер комнаты не должен совпасть даже при одинаковых часах
        assert_ne!(new_room_id(1), new_room_id(2));
    }
}











