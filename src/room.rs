use crate::types::Peer;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

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
