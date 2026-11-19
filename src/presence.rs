//! Онлайн-статус друзей: ping/pong по NodeID.
//!
//! Сервера нет, поэтому «в сети» — это «недавно слышали от него пакет».
//! Фоновый цикл раз в несколько секунд шлёт друзьям `PresencePing`
//! (отправка сама поднимает соединение по NodeID через discovery), а любой
//! ответный пакет — `PresencePong`, чат, заявка — отмечает узел живым.
//! Запись устаревает через [`PRESENCE_TTL`]: пропал друг, не выходя из
//! приложения, — через десяток секунд точка снова серая.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Сколько живёт отметка «в сети» без новых пакетов от узла. Больше пары
/// тактов пингования, чтобы один потерянный пакет не гасил точку.
pub const PRESENCE_TTL: Duration = Duration::from_secs(12);

/// Как часто пингуем друзей, о которых давно ничего не слышали.
pub const PING_PERIOD: Duration = Duration::from_secs(4);

/// Отметки последнего пакета от каждого NodeID.
#[derive(Default)]
pub struct PresenceTracker {
    last_seen: Mutex<HashMap<String, Instant>>,
}

impl PresenceTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Отмечает узел живым (пришёл любой пакет).
    pub fn touch(&self, node_id: &str) {
        if let Ok(mut map) = self.last_seen.lock() {
            map.insert(node_id.to_string(), Instant::now());
        }
    }

    /// В сети ли узел: был пакет не позднее [`PRESENCE_TTL`] назад.
    pub fn is_online(&self, node_id: &str) -> bool {
        self.online_since(node_id).is_some()
    }

    /// Сколько узел не подавал признаков жизни (None — не слышали вовсе).
    pub fn online_since(&self, node_id: &str) -> Option<Duration> {
        let map = self.last_seen.lock().ok()?;
        let seen = map.get(node_id)?;
        let age = seen.elapsed();
        if age <= PRESENCE_TTL {
            Some(age)
        } else {
            None
        }
    }

    /// Забыть узел (друга удалили).
    pub fn forget(&self, node_id: &str) {
        if let Ok(mut map) = self.last_seen.lock() {
            map.remove(node_id);
        }
    }

    /// Узлы, которых пора пинговать: друзья без свежей отметки.
    pub fn stale<'a>(&self, candidates: impl Iterator<Item = &'a str>) -> Vec<String> {
        candidates
            .filter(|id| !self.is_online(id))
            .map(|id| id.to_string())
            .collect()
    }
}

/// Фоновый цикл пинга друзей. Раз в [`PING_PERIOD`] шлёт `PresencePing`
/// тем, о ком давно ничего не слышно; `send_bytes` сам поднимает соединение
/// по NodeID, поэтому друг, только что запустивший приложение, загорится
/// зелёной точкой без всякого «подключиться».
pub fn spawn_ping_loop(
    handle: &tokio::runtime::Handle,
    p2p: std::sync::Arc<crate::network::p2p_iroh::IrohP2pManager>,
    friends: std::sync::Arc<crate::friends::FriendsManager>,
    presence: std::sync::Arc<PresenceTracker>,
    my_id: u64,
) {
    handle.spawn(async move {
        loop {
            tokio::time::sleep(PING_PERIOD).await;
            let node_ids: Vec<String> = friends
                .friends()
                .into_iter()
                .map(|f| f.node_id)
                .collect();
            for node_id in presence.stale(node_ids.iter().map(String::as_str)) {
                let p2p = p2p.clone();
                // Отдельной задачей и с таймаутом: попытка дозвона до
                // выключенного друга иначе висела бы в общем цикле.
                tokio::spawn(async move {
                    let bytes = crate::network::message::serialize_msg(
                        &crate::network::NetworkMessage::PresencePing { sender_id: my_id },
                    );
                    let _ = tokio::time::timeout(
                        Duration::from_secs(6),
                        p2p.send_bytes(&node_id, &bytes),
                    )
                    .await;
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untouched_is_offline() {
        let p = PresenceTracker::new();
        assert!(!p.is_online("abc"));
        assert!(p.online_since("abc").is_none());
    }

    #[test]
    fn touch_makes_online_and_forget_clears() {
        let p = PresenceTracker::new();
        p.touch("abc");
        assert!(p.is_online("abc"));
        p.forget("abc");
        assert!(!p.is_online("abc"));
    }

    #[test]
    fn stale_filters_online_peers() {
        let p = PresenceTracker::new();
        p.touch("online");
        let ids = ["online", "offline"];
        let stale = p.stale(ids.iter().copied());
        assert_eq!(stale, vec!["offline".to_string()]);
    }

    #[test]
    fn ttl_expires_marks() {
        // Старую отметку подкладываем напрямую — sleep в тестах не нужен.
        let p = PresenceTracker::new();
        {
            let mut map = p.last_seen.lock().unwrap();
            map.insert("old".to_string(), Instant::now() - PRESENCE_TTL - Duration::from_secs(1));
        }
        assert!(!p.is_online("old"));
    }
}
