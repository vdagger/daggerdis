use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, Duration};
use tokio::time::{interval};
use crate::room::RoomManager;
use crate::network::NetworkManager;
use crate::protocol::{NetworkMessage, Packet};

pub struct MigrationManager {
    room: Arc<RoomManager>,
    network: Arc<NetworkManager>,
    my_id: u64,
}

impl MigrationManager {
    pub fn new(
        my_id: u64,
        room: Arc<RoomManager>,
        network: Arc<NetworkManager>,
    ) -> Self {
        MigrationManager { room, network, my_id }
    }

    pub async fn start_heartbeat_loop(&self, is_host: Arc<AtomicBool>, room_id: String) {
        let mut interval = interval(Duration::from_millis(500));
        loop {
            interval.tick().await;
            
            if is_host.load(Ordering::Relaxed) {
                // Хост отправляет heartbeat каждые 500мс
                let heartbeat_msg = NetworkMessage {
                    room_id: room_id.clone(),
                    sender_id: self.my_id,
                    packet: Packet::Heartbeat,
                };
                
                let peers = self.room.get_all_peers();
                for peer in peers {
                    let addr = peer.address.parse().unwrap_or_else(|_| "127.0.0.1:0".parse().unwrap());
                    let _ = self.network.send_packet(addr, &heartbeat_msg).await;
                }
            } else {
                // Клиент проверяет последний полученный heartbeat от хоста
                let peers = self.room.get_all_peers();
                let now = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64;
                
                // Ищем хоста и проверяем его последний heartbeat
                for peer in peers {
                    if peer.is_host {
                        if now - peer.last_heartbeat > 2000 {
                            // Хост не отвечает более 2 секунд, запускаем миграцию
                            self.migrate_host(is_host.clone(), room_id.clone()).await;
                            break;
                        }
                    }
                }
            }
        }
    }

    async fn migrate_host(&self, is_host: Arc<AtomicBool>, _room_id: String) {
        let peers = self.room.get_all_peers();
        let mut peer_ids: Vec<u64> = peers.iter().map(|peer| peer.id).collect();
        peer_ids.sort();
        
        // Находим следующий по приоритету ID (минимальный)
        if let Some(next_host_id) = peer_ids.first() {
            if *next_host_id == self.my_id {
                // Становимся новым хостом
                is_host.store(true, Ordering::Relaxed);
                println!("[СЕТЬ] Старый хост отключен. Я стал НОВЫМ ХОСТОМ рации!");
            }
        }
    }
}