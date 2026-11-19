//! Каталог живых трансляций комнаты.
//!
//! Издатель раз в секунду шлёт в `IrohSync` номер своей трансляции
//! (`video_gen`), хост переносит его в релее. Здесь копится то, что
//! анонсировали соседи, — зритель по этой карте находит поток и
//! переподписывается, когда издатель перезапустил трансляцию.
//!
//! Модуль не зависит от iroh: его читают и миграция, и сервис приёма,
//! и GUI.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Сколько живёт анонс без обновления: IrohSync идёт раз в секунду,
/// значит молчание дольше пяти секунд — поток уже не считается живым.
pub const TTL: Duration = Duration::from_secs(5);

struct Live {
    gen: u32,
    seen: Instant,
}

pub struct VideoDirectory {
    my_gen: AtomicU32,
    peers: Mutex<HashMap<String, Live>>,
}

impl VideoDirectory {
    pub fn new() -> Self {
        Self {
            my_gen: AtomicU32::new(0),
            peers: Mutex::new(HashMap::new()),
        }
    }

    /// Поколение собственной трансляции: 0 — не транслируем.
    pub fn my_gen(&self) -> u32 {
        self.my_gen.load(Ordering::Relaxed)
    }

    pub fn set_my_gen(&self, gen: u32) {
        self.my_gen.store(gen, Ordering::Relaxed);
    }

    /// Номер новой трансляции: метка времени, но никогда не 0 (0 = «не
    /// транслирую») и не совпадает с предыдущей — зритель обязан заметить
    /// перезапуск и переподписаться.
    pub fn allocate_gen(&self) -> u32 {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as u32;
        let mut gen = if secs == 0 { 1 } else { secs };
        if gen == self.my_gen() {
            gen = gen.wrapping_add(1);
            if gen == 0 {
                gen = 1;
            }
        }
        gen
    }

    /// Запоминает анонс соседа. gen 0 — «трансляция кончилась», запись убираем.
    pub fn observe(&self, node_id: &str, gen: u32) {
        self.observe_at(node_id, gen, Instant::now());
    }

    fn observe_at(&self, node_id: &str, gen: u32, now: Instant) {
        let Ok(mut peers) = self.peers.lock() else {
            return;
        };
        if gen == 0 {
            peers.remove(node_id);
        } else {
            peers.insert(
                node_id.to_string(),
                Live { gen, seen: now },
            );
        }
    }

    /// Живые трансляции, свежие на момент `now`, по алфавиту NodeID —
    /// порядок стабилен, чтобы GUI не перескакивал между потоками.
    pub fn live(&self, now: Instant) -> Vec<(String, u32)> {
        let Ok(mut peers) = self.peers.lock() else {
            return Vec::new();
        };
        peers.retain(|_, live| {
            live.gen != 0 && now.saturating_duration_since(live.seen) < TTL
        });
        let mut out: Vec<(String, u32)> = peers
            .iter()
            .map(|(node, live)| (node.clone(), live.gen))
            .collect();
        out.sort();
        out
    }

    /// Жив ли конкретный анонс — издатель мог остановиться или перезапуститься.
    pub fn is_live(&self, node_id: &str, gen: u32, now: Instant) -> bool {
        let Ok(peers) = self.peers.lock() else {
            return false;
        };
        peers
            .get(node_id)
            .map(|live| {
                live.gen == gen && now.saturating_duration_since(live.seen) < TTL
            })
            .unwrap_or(false)
    }

    /// Узел ушёл из комнаты — его анонс больше не нужен.
    pub fn clear_peer(&self, node_id: &str) {
        if let Ok(mut peers) = self.peers.lock() {
            peers.remove(node_id);
        }
    }

    pub fn clear_peers(&self) {
        if let Ok(mut peers) = self.peers.lock() {
            peers.clear();
        }
    }
}

impl Default for VideoDirectory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_gen_means_off_air() {
        let dir = VideoDirectory::new();
        let now = Instant::now();
        dir.observe("node-a", 5);
        assert_eq!(dir.live(now), vec![("node-a".to_string(), 5)]);
        dir.observe("node-a", 0);
        assert!(dir.live(now).is_empty());
        assert!(!dir.is_live("node-a", 5, now));
    }

    #[test]
    fn ttl_expires_silent_announcement() {
        let dir = VideoDirectory::new();
        let t0 = Instant::now();
        dir.observe_at("node-a", 3, t0);
        assert_eq!(dir.live(t0 + TTL - Duration::from_millis(1)).len(), 1);
        assert!(dir.live(t0 + TTL).is_empty());
        assert!(!dir.is_live("node-a", 3, t0 + TTL));
    }

    #[test]
    fn new_gen_replaces_old_and_sorts() {
        let dir = VideoDirectory::new();
        let now = Instant::now();
        dir.observe("node-b", 1);
        dir.observe("node-a", 2);
        dir.observe("node-b", 7);
        assert_eq!(
            dir.live(now),
            vec![("node-a".to_string(), 2), ("node-b".to_string(), 7)]
        );
        assert!(!dir.is_live("node-b", 1, now));
        assert!(dir.is_live("node-b", 7, now));
    }

    #[test]
    fn my_gen_and_allocate() {
        let dir = VideoDirectory::new();
        assert_eq!(dir.my_gen(), 0);
        let gen = dir.allocate_gen();
        assert_ne!(gen, 0);
        dir.set_my_gen(gen);
        assert_eq!(dir.my_gen(), gen);
        // повторный запуск не выдаёт тот же номер
        let again = dir.allocate_gen();
        assert_ne!(again, 0);
        assert_ne!(again, gen);
        dir.set_my_gen(0);
        assert_eq!(dir.my_gen(), 0);
    }

    #[test]
    fn clear_peer_removes_announcement() {
        let dir = VideoDirectory::new();
        dir.observe("node-a", 4);
        dir.clear_peer("node-a");
        assert!(dir.live(Instant::now()).is_empty());
    }
}
