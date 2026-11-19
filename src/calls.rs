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
        let accepted = {
            let Ok(mut phase) = self.phase.lock() else {
                return false;
            };
            if let CallPhase::Incoming {
                peer,
                nick,
                call_id,
                ..
            } = &*phase
            {
                let call = (peer.clone(), *call_id);
                *phase = CallPhase::Active {
                    peer: peer.clone(),
                    nick: nick.clone(),
                    call_id: *call_id,
                    started: Instant::now(),
                };
                Some(call)
            } else {
                None
            }
        };
        let Some((peer, call_id)) = accepted else {
            return false;
        };
        // Согласие получено — входим в эфир, снимаем блок и подтверждаем
        // приём несколько раз: сообщение с Accept может потеряться.
        self.room.join();
        self.unblock(&peer);
        self.send_accept_repeat(&peer, call_id);
        true
    }

    pub fn decline_incoming(&self) -> bool {
        let declined = {
            let Ok(mut phase) = self.phase.lock() else {
                return false;
            };
            if let CallPhase::Incoming { peer, call_id, .. } = &*phase {
                let call = (peer.clone(), *call_id);
                *phase = CallPhase::Idle;
                Some(call)
            } else {
                None
            }
        };
        let Some((peer, call_id)) = declined else {
            return false;
        };
        self.send_decline(&peer, call_id, REASON_DECLINED);
        self.mark_declined(&peer);
        self.block(&peer, BLOCK_AFTER);
        self.disconnect_later(&peer, 800);
        true
    }

    /// Отменить свой звонок, пока не ответили
    pub fn cancel_outgoing(&self) {
        let cancelled = {
            let Ok(mut phase) = self.phase.lock() else {
                return;
            };
            if let CallPhase::Outgoing { peer, call_id, .. } = &*phase {
                let call = (peer.clone(), *call_id);
                *phase = CallPhase::Idle;
                Some(call)
            } else {
                None
            }
        };
        let Some((peer, call_id)) = cancelled else {
            return;
        };
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

    /// Завершить состоявшийся разговор
    pub fn end_active(&self) {
        let ended = {
            let Ok(mut phase) = self.phase.lock() else {
                return;
            };
            if let CallPhase::Active { peer, call_id, .. } = &*phase {
                let call = (peer.clone(), *call_id);
                *phase = CallPhase::Idle;
                Some(call)
            } else {
                None
            }
        };
        let Some((peer, call_id)) = ended else {
            return;
        };
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

    // ————— Входящие сообщения (из iroh_service) —————

    pub fn on_invite(&self, peer: &str, nick: &str, call_id: u32) {
        enum Act {
            None,
            Decline(u8),
            /// Встречный звонок принят — подтвердить по call_id
            Accept(u32),
        }
        let act = {
            let Ok(mut phase) = self.phase.lock() else {
                return;
            };
            match &mut *phase {
                CallPhase::Idle => {
                    if self.declined_now(peer) {
                        // Повторный Invite после отказа — отвечаем отказом снова
                        Act::Decline(REASON_DECLINED)
                    } else {
                        *phase = CallPhase::Incoming {
                            peer: peer.to_string(),
                            nick: nick.to_string(),
                            call_id,
                            started: Instant::now(),
                        };
                        Act::None
                    }
                }
                CallPhase::Incoming {
                    peer: p,
                    nick: n,
                    call_id: c,
                    started,
                } => {
                    if p == peer {
                        // Повтор нашего же приглашения: звеним дальше, но новый
                        // call_id означает, что собеседник перезвонил заново
                        if *c != call_id {
                            *n = nick.to_string();
                            *c = call_id;
                            *started = Instant::now();
                        }
                        Act::None
                    } else {
                        Act::Decline(REASON_BUSY)
                    }
                }
                CallPhase::Outgoing {
                    peer: p, nick: n, ..
                } => {
                    if p == peer {
                        // Оба позвонили одновременно: принимаем встречный звонок
                        let peer = p.clone();
                        let nick = n.clone();
                        *phase = CallPhase::Active {
                            peer,
                            nick,
                            call_id,
                            started: Instant::now(),
                        };
                        Act::Accept(call_id)
                    } else {
                        Act::Decline(REASON_BUSY)
                    }
                }
                CallPhase::Active { peer: p, call_id: c, .. } => {
                    if p == peer {
                        // Повторный Invite: звонящий не получил Accept
                        Act::Accept(*c)
                    } else {
                        Act::Decline(REASON_BUSY)
                    }
                }
            }
        };
        match act {
            Act::None => {}
            Act::Decline(reason) => self.send_decline(peer, call_id, reason),
            Act::Accept(cid) => {
                println!("[ЗВОНКИ] Встречный звонок с {} — принимаем", short(peer));
                self.room.join();
                self.unblock(peer);
                self.send_accept_repeat(peer, cid);
            }
        }
    }

    /// `call_id` должен совпасть: иначе запоздавший Accept от предыдущего
    /// звонка поднимал бы новый (и оверлей «В звонке» без собеседника).
    pub fn on_accept(&self, peer: &str, call_id: u32) {
        let activated = {
            let Ok(mut phase) = self.phase.lock() else {
                return;
            };
            match &mut *phase {
                CallPhase::Outgoing {
                    peer: p,
                    nick,
                    call_id: c,
                    ..
                } if p == peer && *c == call_id => {
                    let nick = nick.clone();
                    *phase = CallPhase::Active {
                        peer: p.clone(),
                        nick,
                        call_id,
                        started: Instant::now(),
                    };
                    true
                }
                _ => false,
            }
        };
        if activated {
            // Мы вошли в комнату собеседника, он — хост
            self.room.join();
            self.is_host.store(false, Ordering::SeqCst);
            self.unblock(peer);
            println!("[ЗВОНКИ] {} принял звонок", short(peer));
        }
    }

    pub fn on_decline(&self, peer: &str, call_id: u32, reason: u8) {
        let was_outgoing = {
            let Ok(mut phase) = self.phase.lock() else {
                return;
            };
            match &mut *phase {
                CallPhase::Outgoing {
                    peer: p,
                    call_id: c,
                    ..
                } if p == peer && *c == call_id => {
                    *phase = CallPhase::Idle;
                    true
                }
                _ => false,
            }
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

    pub fn on_cancel(&self, peer: &str, call_id: u32) {
        let nick = {
            let Ok(mut phase) = self.phase.lock() else {
                return;
            };
            match &mut *phase {
                CallPhase::Incoming {
                    peer: p,
                    nick,
                    call_id: c,
                    ..
                } if p == peer && *c == call_id => {
                    let nick = nick.clone();
                    *phase = CallPhase::Idle;
                    Some(nick)
                }
                _ => None,
            }
        };
        if let Some(nick) = nick {
            self.set_notice(format!("Пропущенный звонок: {nick}"));
            self.block(peer, BLOCK_AFTER);
            self.disconnect_later(peer, 400);
        }
    }

    pub fn on_end(&self, peer: &str, call_id: u32) {
        let ended = {
            let Ok(mut phase) = self.phase.lock() else {
                return;
            };
            match &mut *phase {
                CallPhase::Active {
                    peer: p,
                    call_id: c,
                    ..
                }
                | CallPhase::Incoming {
                    peer: p,
                    call_id: c,
                    ..
                }
                | CallPhase::Outgoing {
                    peer: p,
                    call_id: c,
                    ..
                } if p == peer && *c == call_id => {
                    *phase = CallPhase::Idle;
                    true
                }
                _ => false,
            }
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
        let now = Instant::now();
        let mut resend: Option<(String, u32)> = None;
        let mut timed_out: Option<(String, String, bool)> = None; // peer, nick, исходящий
        {
            let Ok(mut phase) = self.phase.lock() else {
                return;
            };
            match &mut *phase {
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
                *phase = CallPhase::Idle;
            }
        }

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
                // Без блокировки его IrohSync через секунду вернул бы его в
                // комнату, хотя звонок так и не состоялся.
                self.block(&peer, BLOCK_AFTER);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> CallManager {
        CallManager::new(
            1,
            "Я".to_string(),
            None,
            Arc::new(RoomManager::new(1)),
            Arc::new(AtomicBool::new(true)),
        )
    }

    fn call_id_of(m: &CallManager) -> u32 {
        match &*m.phase.lock().unwrap() {
            CallPhase::Outgoing { call_id, .. }
            | CallPhase::Incoming { call_id, .. }
            | CallPhase::Active { call_id, .. } => *call_id,
            CallPhase::Idle => panic!("фаза Idle"),
        }
    }

    #[test]
    fn outgoing_accept_requires_matching_call_id() {
        let m = manager();
        assert!(m.start_outgoing("peerA", "Друг", "Я"));
        assert!(matches!(m.view(), CallView::Outgoing { .. }));

        let id = call_id_of(&m);
        // запоздавший Accept от прошлого звонка не должен поднимать новый
        m.on_accept("peerA", id ^ u32::MAX);
        assert!(matches!(m.view(), CallView::Outgoing { .. }));

        m.on_accept("peerA", id);
        assert!(matches!(m.view(), CallView::Active { .. }));
        assert!(!m.is_blocked("peerA"));
    }

    #[test]
    fn second_call_refused_while_busy() {
        let m = manager();
        assert!(m.start_outgoing("peerA", "А", "Я"));
        assert!(!m.start_outgoing("peerB", "Б", "Я"));
        assert_eq!(m.take_notice().as_deref(), Some("Вы уже в звонке"));
    }

    #[test]
    fn decline_blocks_peer() {
        let m = manager();
        m.start_outgoing("peerA", "А", "Я");
        let id = call_id_of(&m);
        m.on_decline("peerA", id, REASON_DECLINED);
        assert!(matches!(m.view(), CallView::Idle));
        assert!(m.is_blocked("peerA"));
        assert_eq!(m.take_notice().as_deref(), Some("Звонок отклонён"));
    }

    #[test]
    fn busy_reason_notice() {
        let m = manager();
        m.start_outgoing("peerA", "А", "Я");
        let id = call_id_of(&m);
        m.on_decline("peerA", id, REASON_BUSY);
        assert_eq!(m.take_notice().as_deref(), Some("Собеседник занят"));
    }

    #[test]
    fn incoming_accept_and_end() {
        let m = manager();
        m.on_invite("peerB", "Б", 77);
        assert!(matches!(m.view(), CallView::Incoming { .. }));
        assert!(m.is_blocked("peerB"));

        assert!(m.accept_incoming());
        assert!(matches!(m.view(), CallView::Active { .. }));
        assert!(!m.is_blocked("peerB"));

        m.end_active();
        assert!(matches!(m.view(), CallView::Idle));
        assert!(m.is_blocked("peerB"), "после завершения собеседник вне комнаты");
    }

    #[test]
    fn cancel_with_wrong_call_id_ignored() {
        let m = manager();
        m.on_invite("peerB", "Б", 5);
        m.on_cancel("peerB", 6);
        assert!(matches!(m.view(), CallView::Incoming { .. }));
        m.on_cancel("peerB", 5);
        assert!(matches!(m.view(), CallView::Idle));
    }

    #[test]
    fn busy_declines_second_invite() {
        let m = manager();
        m.on_invite("peerB", "Б", 1);
        m.on_invite("peerC", "В", 2);
        match m.view() {
            CallView::Incoming { nick, .. } => assert_eq!(nick, "Б"),
            other => panic!("ожидали Incoming, получили {other:?}"),
        }
    }

    #[test]
    fn simultaneous_call_becomes_active() {
        let m = manager();
        m.start_outgoing("peerA", "А", "Я");
        m.on_invite("peerA", "А", 42);
        assert!(matches!(m.view(), CallView::Active { .. }));
    }

    #[test]
    fn decline_repeats_after_own_refusal() {
        let m = manager();
        m.on_invite("peerB", "Б", 9);
        assert!(m.decline_incoming());
        assert!(matches!(m.view(), CallView::Idle));
        // звонящий повторяет Invite — снова отказ, карточка не всплывает
        m.on_invite("peerB", "Б", 10);
        assert!(matches!(m.view(), CallView::Idle));
    }

    #[test]
    fn stale_end_ignored() {
        let m = manager();
        m.on_invite("peerB", "Б", 11);
        assert!(m.accept_incoming());
        m.on_end("peerB", 12);
        assert!(matches!(m.view(), CallView::Active { .. }));
        m.on_end("peerB", 11);
        assert!(matches!(m.view(), CallView::Idle));
    }
}
