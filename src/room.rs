use crate::types::Peer;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Хост по списку соседей: минимальный id среди помеченных `is_host`.
/// Выборы миграции берут по тому же правилу, поэтому все сходятся на одном.
fn select_host(peers: &[Peer]) -> Option<Peer> {
    peers
        .iter()
        .filter(|peer| peer.is_host)
        .min_by_key(|peer| peer.id)
        .cloned()
}

pub struct RoomManager {
    my_id: u64,
    peers: Mutex<Vec<Peer>>,
    /// Пользователь сам вышел из эфира: IrohSync/Heartbeat/AudioFrame от
    /// собеседников игнорируются, иначе они за секунду вернули бы его в комнату.
    /// Снимается явным «Войти» или принятым/начатым звонком.
    left: AtomicBool,
}

impl RoomManager {
    pub fn new(my_id: u64) -> Self {
        RoomManager {
            my_id,
            peers: Mutex::new(Vec::new()),
            left: AtomicBool::new(false),
        }
    }

    /// Явно выйти из эфира: убрать всех и не пускать обратно до нового входа
    pub fn leave(&self) {
        self.left.store(true, Ordering::SeqCst);
        self.peers.lock().unwrap().clear();
    }

    /// Вернуться в эфир (нажали «Войти» или начали/приняли звонок)
    pub fn join(&self) {
        self.left.store(false, Ordering::SeqCst);
    }

    pub fn is_left(&self) -> bool {
        self.left.load(Ordering::SeqCst)
    }

    pub fn update_peer(&self, peer: Peer) {
        let mut peers = self.peers.lock().unwrap();
        if let Some(existing_peer) = peers.iter_mut().find(|p| p.id == peer.id) {
            existing_peer.last_heartbeat = peer.last_heartbeat;
            existing_peer.address = peer.address;
            existing_peer.is_host = peer.is_host;
        } else {
            peers.push(peer);
        }
    }

    pub fn remove_peer(&self, id: u64) {
        let mut peers = self.peers.lock().unwrap();
        peers.retain(|peer| peer.id != id);
    }

    pub fn get_all_addresses(&self) -> Vec<String> {
        let peers = self.peers.lock().unwrap();
        peers.iter().map(|peer| peer.address.clone()).collect()
    }

    /// Хост комнаты: наименьший id среди соседей и себя.
    ///
    /// Правило детерминированное — каждый узел считает его по одному и тому же
    /// списку, поэтому все сходятся к одному хосту без обмена сообщениями.
    /// Флаг `is_host` в пакетах для этого не нужен: он у каждого свой и в момент
    /// входа выставлен у всех.
    pub fn host_id_with_me(&self, my_id: u64) -> u64 {
        let peers = self.peers.lock().unwrap();
        peers
            .iter()
            .map(|p| p.id)
            .chain(std::iter::once(my_id))
            .min()
            .unwrap_or(my_id)
    }

    /// id хоста комнаты: наименьший среди помеченных `is_host`.
    ///
    /// По этому правилу сходятся все узлы: `is_host=true` у каждого по умолчанию,
    /// поэтому «хост» — не флаг, а наименьший id. Видео на это опирается: хост не
    /// подписывается на чужие потоки, и если хостом себя считает каждый, то
    /// трансляцию не смотрит никто.
    pub fn host_id(&self) -> Option<u64> {
        let peers = self.peers.lock().unwrap();
        peers.iter().filter(|p| p.is_host).map(|p| p.id).min()
    }

    /// Хост комнаты среди соседей: наименьший id из помеченных `is_host` —
    /// тот же порядок, по которому выбирает миграция. Своего узла в `peers`
    /// нет, поэтому `None` значит «хостом считаю себя или хоста ещё нет».
    pub fn host_peer(&self) -> Option<Peer> {
        let peers = self.peers.lock().unwrap();
        select_host(&peers)
    }

    // Добавляем публичные методы для доступа к приватным полям
    pub fn get_all_peers(&self) -> Vec<Peer> {
        let peers = self.peers.lock().unwrap();
        peers.clone()
    }

    pub fn get_my_id(&self) -> u64 {
        self.my_id
    }

    pub fn set_host(&self, new_host_id: u64) {
        let mut peers = self.peers.lock().unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        for peer in peers.iter_mut() {
            peer.is_host = peer.id == new_host_id;
            if peer.id == new_host_id {
                // Сбрасываем таймер для нового хоста, чтобы избежать ложных повторных выборов
                peer.last_heartbeat = now;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(id: u64, is_host: bool) -> Peer {
        Peer {
            id,
            address: format!("node-{id}"),
            is_host,
            last_heartbeat: 0,
        }
    }

    #[test]
    fn select_host_picks_lowest_id() {
        let peers = vec![peer(9, true), peer(5, true), peer(3, false)];
        assert_eq!(select_host(&peers).map(|p| p.id), Some(5));
    }

    #[test]
    fn select_host_none_when_nobody_is_host() {
        assert!(select_host(&[peer(3, false)]).is_none());
        assert!(select_host(&[]).is_none());
    }

    #[test]
    fn host_is_lowest_id_including_me() {
        let room = RoomManager::new(8);
        room.update_peer(peer(1, true));
        room.update_peer(peer(9, false));
        assert_eq!(room.host_id_with_me(8), 1, "хост — меньший id комнаты");
        let room2 = RoomManager::new(3);
        room2.update_peer(peer(5, true));
        assert_eq!(room2.host_id_with_me(3), 3, "я сам могу быть хостом");
        let empty = RoomManager::new(42);
        assert_eq!(empty.host_id_with_me(42), 42, "один в комнате — хост");
    }

    #[test]
    fn host_id_is_lowest_among_claimants() {
        let room = RoomManager::new(7);
        room.update_peer(peer(9, true));
        room.update_peer(peer(4, true));
        room.update_peer(peer(3, false));
        assert_eq!(room.host_id(), Some(4), "хост — меньший id среди is_host");
    }
}


