//! Звонки как в Discord: приглашение → «Принять» / «Отклонить» → разговор.
//!
//! Кнопка «Позвонить» больше не затаскивает друга в комнату молча: собеседник
//! получает `CallInvite`, слышит гудок и решает сам. Пока звонок не принят,
//! приглашающий заблокирован от добавления в комнату — иначе авто-синхронизация
//! mesh (IrohSync раз в секунду) добавляла бы его в участники в обход согласия.

use crate::network::message::serialize_msg;
use crate::network::p2p_iroh::IrohP2pManager;
use crate::network::NetworkMessage;
use crate::room::RoomManager;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Сколько ждём ответа на исходящий звонок
const OUTGOING_TIMEOUT: Duration = Duration::from_secs(35);
/// Сколько звоним входящий, прежде чем считать пропущенным
const INCOMING_TIMEOUT: Duration = Duration::from_secs(45);
/// Датаграммы теряются: приглашение повторяем, пока не ответят
const INVITE_RESEND: Duration = Duration::from_millis(1500);
/// После отказа/отмены/завершения держим собеседника вне комнаты. Долго, а не
/// «пока рвётся соединение»: иначе через 45 с его IrohSync снова затаскивал
/// нас в комнату, и завершённый звонок «возобновлялся» сам.
/// Снимается явно: новым звонком или нажатием «Войти».
const BLOCK_AFTER: Duration = Duration::from_secs(3600);
/// Сколько отвечаем отказом на повторные Invite после того, как сами отказали:
/// звонящий повторяет приглашение, пока не ответят.
const DECLINED_AFTER: Duration = Duration::from_secs(45);
/// Причина в CallDecline
const REASON_DECLINED: u8 = 0;
const REASON_BUSY: u8 = 1;
const REASON_TIMEOUT: u8 = 2;

#[derive(Clone, Debug)]
enum CallPhase {
    Idle,
    Outgoing {
        peer: String,
        nick: String,
        call_id: u32,
        started: Instant,
        last_invite: Instant,
    },
    Incoming {
        peer: String,
        nick: String,
        call_id: u32,
        started: Instant,
    },
    Active {
        peer: String,
        nick: String,
        call_id: u32,
        started: Instant,
    },
}

/// Состояние для оверлея — без сети и блокировок
#[derive(Clone, Debug)]
pub enum CallView {
    Idle,
    Outgoing { nick: String, elapsed: Duration },
    Incoming { nick: String, elapsed: Duration },
    Active { nick: String, elapsed: Duration },
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

    /// Звонящий или приглашающий не должен попасть в комнату до согласия
    pub fn is_blocked(&self, peer: &str) -> bool {
        if let Ok(phase) = self.phase.lock() {
            match &*phase {
                CallPhase::Outgoing { peer: p, .. } | CallPhase::Incoming { peer: p, .. }
                    if p == peer =>
                {
                    return true;
                }
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
            CallPhase::Outgoing { nick, started, .. } => CallView::Outgoing {
                nick: nick.clone(),
                elapsed: now.duration_since(*started),
            },
            CallPhase::Incoming { nick, started, .. } => CallView::Incoming {
                nick: nick.clone(),
                elapsed: now.duration_since(*started),
            },
            CallPhase::Active { nick, started, .. } => CallView::Active {
                nick: nick.clone(),
                elapsed: now.duration_since(*started),
            },
        }
    }

    /// 0 — тихо, 1 — входящий звонок, 2 — гудок «звоним…»
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

    /// Начать звонок другу
    pub fn start_outgoing(&self, peer: &str, peer_nick: &str, my_nick: &str) -> bool {
        let call_id = new_call_id();
        {
            let Ok(mut phase) = self.phase.lock() else {
                return false;
            };
            if !matches!(*phase, CallPhase::Idle) {
                self.set_notice("Вы уже в звонке".to_string());
                return false;
            }
            *phase = CallPhase::Outgoing {
                peer: peer.to_string(),
                nick: peer_nick.to_string(),
                call_id,
                started: Instant::now(),
                last_invite: Instant::now(),
            };
        }
        // Новый звонок — явное возвращение в эфир
        self.room.join();
        self.unblock(peer);
        self.send_to(
            peer,
            NetworkMessage::CallInvite {
                sender_id: self.my_id,
                call_id,
                nick: my_nick.to_string(),
            },
        );
        true
    }

    /// Принять входящий звонок
    pub fn accept_incoming(&self) -> bool {
        match self.take_phase() {
            Some(CallPhase::Incoming {
                peer,
                nick,
                call_id,
                ..
            }) => {
                self.put_phase(CallPhase::Active {
                    peer: peer.clone(),
                    nick,
                    call_id,
                    started: Instant::now(),
                });
                // Согласие получено — входим в эфир, снимаем блок и подтверждаем
                // приём несколько раз: сообщение с Accept может потеряться.
                self.room.join();
                self.unblock(&peer);
                self.send_accept_repeat(&peer, call_id);
                true
            }
            Some(other) => {
                self.put_phase(other);
                false
            }
            None => false,
        }
    }

    pub fn decline_incoming(&self) -> bool {
        match self.take_phase() {
            Some(CallPhase::Incoming { peer, call_id, .. }) => {
                self.send_decline(&peer, call_id, REASON_DECLINED);
                self.mark_declined(&peer);
                self.block(&peer, BLOCK_AFTER);
                self.disconnect_later(&peer, 800);
                true
            }
            Some(other) => {
                self.put_phase(other);
                false
            }
            None => false,
        }
    }

    /// Отменить свой звонок, пока не ответили
    pub fn cancel_outgoing(&self) {
        let Some(current) = self.take_phase() else {
            return;
        };
        match current {
            CallPhase::Outgoing { peer, call_id, .. } => {
                self.send_to(
                    &peer,
                    NetworkMessage::CallCancel {
                        sender_id: self.my_id,
                        call_id,
                    },
                );
                self.block(&peer, BLOCK_AFTER);
                self.drop_from_room(&peer);
                self.disconnect_later(&peer, 600);
            }
            other => self.put_phase(other),
        }
    }

    /// Завершить состоявшийся разговор
    pub fn end_active(&self) {
        let Some(current) = self.take_phase() else {
            return;
        };
        match current {
            CallPhase::Active { peer, call_id, .. } => {
                self.send_to(
                    &peer,
                    NetworkMessage::CallEnd {
                        sender_id: self.my_id,
                        call_id,
                    },
                );
                // Завершили — собеседник больше не участник эфира, пока не
                // позвонят или не нажмут «Войти». Иначе его Heartbeat/IrohSync
                // через секунду возвращал его в комнату.
                self.block(&peer, BLOCK_AFTER);
                self.drop_from_room(&peer);
                self.disconnect_later(&peer, 600);
            }
            other => self.put_phase(other),
        }
    }

    // ————— Входящие сообщения (из iroh_service) —————

    pub fn on_invite(&self, peer: &str, nick: &str, call_id: u32) {
        let Some(current) = self.take_phase() else {
            return;
        };
        match current {
            CallPhase::Idle => {
                if self.declined_now(peer) {
                    // Повторный Invite после отказа — отвечаем отказом снова
                    self.send_decline(peer, call_id, REASON_DECLINED);
                } else {
                    self.put_phase(CallPhase::Incoming {
                        peer: peer.to_string(),
                        nick: nick.to_string(),
                        call_id,
                        started: Instant::now(),
                    });
                }
            }
            CallPhase::Incoming {
                peer: p,
                nick: n,
                call_id: c,
                started,
            } => {
                if p == peer {
                    // Повтор нашего же приглашения: звеним дальше
                    if c == call_id {
                        self.put_phase(CallPhase::Incoming {
                            peer: p,
                            nick: n,
                            call_id: c,
                            started,
                        });
                    } else {
                        self.put_phase(CallPhase::Incoming {
                            peer: p,
                            nick: nick.to_string(),
                            call_id,
                            started: Instant::now(),
                        });
                    }
                } else {
                    self.put_phase(CallPhase::Incoming {
                        peer: p,
                        nick: n,
                        call_id: c,
                        started,
                    });
                    self.send_decline(peer, call_id, REASON_BUSY);
                }
            }
            CallPhase::Outgoing {
                peer: p,
                nick: n,
                call_id: c,
                started,
                last_invite,
            } => {
                if p == peer {
                    // Оба позвонили одновременно: принимаем встречный звонок
                    self.put_phase(CallPhase::Active {
                        peer: p,
                        nick: n,
                        call_id,
                        started: Instant::now(),
                    });
                    println!("[ЗВОНКИ] Встречный звонок с {} — принимаем", short(peer));
                    self.room.join();
                    self.unblock(peer);
                    self.send_accept_repeat(peer, call_id);
                } else {
                    self.put_phase(CallPhase::Outgoing {
                        peer: p,
                        nick: n,
                        call_id: c,
                        started,
                        last_invite,
                    });
                    self.send_decline(peer, call_id, REASON_BUSY);
                }
            }
            CallPhase::Active {
                peer: p,
                nick: n,
                call_id: c,
                started,
            } => {
                if p == peer {
                    self.put_phase(CallPhase::Active {
                        peer: p,
                        nick: n,
                        call_id: c,
                        started,
                    });
                    // Повторный Invite: звонящий не получил Accept
                    self.send_accept_repeat(peer, c);
                } else {
                    self.put_phase(CallPhase::Active {
                        peer: p,
                        nick: n,
                        call_id: c,
                        started,
                    });
                    self.send_decline(peer, call_id, REASON_BUSY);
                }
            }
        }
    }

    pub fn on_accept(&self, peer: &str) {
        let activated = match self.take_phase() {
            Some(CallPhase::Outgoing {
                peer: p,
                nick,
                call_id,
                ..
            }) if p == peer => {
                self.put_phase(CallPhase::Active {
                    peer: p,
                    nick,
                    call_id,
                    started: Instant::now(),
                });
                true
            }
            Some(other) => {
                self.put_phase(other);
                false
            }
            None => false,
        };
        if activated {
            // Мы вошли в комнату собеседника, он — хост
            self.room.join();
            self.is_host.store(false, Ordering::SeqCst);
            self.unblock(peer);
            println!("[ЗВОНКИ] {} принял звонок", short(peer));
        }
    }

    pub fn on_decline(&self, peer: &str, reason: u8) {
        let was_outgoing = match self.take_phase() {
            Some(CallPhase::Outgoing { peer: p, .. }) if p == peer => true,
            Some(other) => {
                self.put_phase(other);
                false
            }
            None => false,
        };
        if !was_outgoing {
            return;
        }
        if reason == REASON_BUSY {
            self.set_notice("Собеседник занят".to_string());
        } else if reason == REASON_TIMEOUT {
            self.set_notice("Не ответил".to_string());
        } else {
            self.set_notice("Звонок отклонён".to_string());
        }
        self.block(peer, BLOCK_AFTER);
        self.drop_from_room(peer);
        self.disconnect_later(peer, 400);
    }

    pub fn on_cancel(&self, peer: &str) {
        let nick = match self.take_phase() {
            Some(CallPhase::Incoming {
                peer: p, nick, ..
            }) if p == peer => Some(nick),
            Some(other) => {
                self.put_phase(other);
                None
            }
            None => None,
        };
        if let Some(nick) = nick {
            self.set_notice(format!("Пропущенный звонок: {nick}"));
            self.block(peer, BLOCK_AFTER);
            self.disconnect_later(peer, 400);
        }
    }

    pub fn on_end(&self, peer: &str) {
        let ended = match self.take_phase() {
            Some(
                CallPhase::Active { peer: p, .. }
                | CallPhase::Incoming { peer: p, .. }
                | CallPhase::Outgoing { peer: p, .. },
            ) if p == peer => true,
            Some(other) => {
                self.put_phase(other);
                false
            }
            None => false,
        };
        if ended {
            self.set_notice("Звонок завершён".to_string());
            self.block(peer, BLOCK_AFTER);
            self.drop_from_room(peer);
            self.disconnect_later(peer, 100);
        }
    }

    /// Раз в кадр: повтор приглашений и таймауты
    pub fn tick(&self) {
        let Some(mut current) = self.take_phase() else {
            return;
        };
        let now = Instant::now();
        let mut resend: Option<(String, u32)> = None;
        let mut timed_out: Option<(String, String, bool)> = None; // peer, nick, исходящий

        match &mut current {
            CallPhase::Outgoing {
                peer,
                nick,
                call_id,
                started,
                last_invite,
            } => {
                if now.duration_since(*started) >= OUTGOING_TIMEOUT {
                    timed_out = Some((peer.clone(), nick.clone(), true));
                } else if now.duration_since(*last_invite) >= INVITE_RESEND {
                    *last_invite = now;
                    resend = Some((peer.clone(), *call_id));
                }
            }
            CallPhase::Incoming {
                peer,
                nick,
                started,
                ..
            } => {
                if now.duration_since(*started) >= INCOMING_TIMEOUT {
                    timed_out = Some((peer.clone(), nick.clone(), false));
                }
            }
            _ => {}
        }

        if timed_out.is_some() {
            current = CallPhase::Idle;
        }
        self.put_phase(current);

        if let Some((peer, call_id)) = resend {
            let my_nick = self.my_nick.lock().map(|n| n.clone()).unwrap_or_default();
            self.send_to(
                &peer,
                NetworkMessage::CallInvite {
                    sender_id: self.my_id,
                    call_id,
                    nick: my_nick,
                },
            );
        }

        if let Some((peer, nick, was_outgoing)) = timed_out {
            if was_outgoing {
                self.set_notice(format!("{nick} не ответил"));
                self.drop_from_room(&peer);
                self.disconnect_later(&peer, 400);
            } else {
                self.set_notice(format!("Пропущенный звонок: {nick}"));
                self.send_decline(&peer, 0, REASON_TIMEOUT);
                self.mark_declined(&peer);
                self.block(&peer, BLOCK_AFTER);
                self.disconnect_later(&peer, 400);
            }
        }
    }

    // ————— Внутреннее —————

    fn take_phase(&self) -> Option<CallPhase> {
        self.phase
            .lock()
            .ok()
            .map(|mut g| std::mem::replace(&mut *g, CallPhase::Idle))
    }

    fn put_phase(&self, phase: CallPhase) {
        if let Ok(mut g) = self.phase.lock() {
            *g = phase;
        }
    }

    fn send_to(&self, peer: &str, msg: NetworkMessage) {
        let Some(p2p) = self.p2p.clone() else {
            eprintln!("[ЗВОНКИ] Сеть не запущена — сообщение не отправлено");
            return;
        };
        let peer = peer.to_string();
        let bytes = serialize_msg(&msg);
        let Ok(rt) = tokio::runtime::Handle::try_current() else {
            eprintln!("[ЗВОНКИ] Нет tokio-рантайма для отправки");
            return;
        };
        rt.spawn(async move {
            let connected = p2p.get_connected_peers().await;
            if !connected.iter().any(|p| p == &peer) {
                if let Err(e) = p2p.connect_to_peer(&peer).await {
                    eprintln!(
                        "[ЗВОНКИ] Не удалось подключиться к {}: {}",
                        short(&peer),
                        e
                    );
                    return;
                }
            }
            if let Err(e) = p2p.send_bytes(&peer, &bytes).await {
                eprintln!("[ЗВОНКИ] Ошибка отправки {}: {}", short(&peer), e);
            }
        });
    }

    fn send_decline(&self, peer: &str, call_id: u32, reason: u8) {
        self.send_to(
            peer,
            NetworkMessage::CallDecline {
                sender_id: self.my_id,
                call_id,
                reason,
            },
        );
        if reason == REASON_DECLINED {
            self.block(peer, BLOCK_AFTER);
        }
    }

    /// Accept шлём трижды: один потерянный датаграм не должен оставить
    /// звонящего «звонить» уже после ответа.
    fn send_accept_repeat(&self, peer: &str, call_id: u32) {
        let Some(p2p) = self.p2p.clone() else {
            return;
        };
        let peer = peer.to_string();
        let bytes = serialize_msg(&NetworkMessage::CallAccept {
            sender_id: self.my_id,
            call_id,
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
    pub fn unblock_peer(&self, peer: &str) {
        self.unblock(peer);
    }
}

fn new_call_id() -> u32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    nanos | 1
}

fn short(node_id: &str) -> &str {
    &node_id[..node_id.len().min(10)]
}
