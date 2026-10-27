use std::sync::Mutex;
use crate::types::Peer;

pub struct RoomManager {
    my_id: u64,
    peers: Mutex<Vec<Peer>>,
}

impl RoomManager {
    pub fn new(my_id: u64) -> Self {
        RoomManager {
            my_id,
            peers: Mutex::new(Vec::new()),
        }
    }

    pub fn update_peer(&self, peer: Peer) {
        let mut peers = self.peers.lock().unwrap();
        // Ищем либо по точному ID, либо по адресу, если ID был 0 (заглушка пробива NAT)
        if let Some(existing_peer) = peers.iter_mut().find(|p| p.id == peer.id || (p.id == 0 && p.address == peer.address)) {
            existing_peer.id = peer.id; // перезаписываем временный ID 0 на реальный ID
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

    pub fn get_next_host_id(&self) -> Option<u64> {
        let peers = self.peers.lock().unwrap();
        if peers.is_empty() {
            return Some(self.my_id);
        }

        let min_peer_id = peers.iter().map(|peer| peer.id).min().unwrap();
        
        if min_peer_id < self.my_id {
            Some(min_peer_id)
        } else {
            Some(self.my_id)
        }
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