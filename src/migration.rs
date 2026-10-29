use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, Duration};
use tokio::time::{interval};
use crate::room::RoomManager;
use crate::network::NetworkManager;
use crate::network::NetworkMessage;

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
        let mut interval = interval(Duration::from_millis(1000));
        loop {
            interval.tick().await;
            
            let my_is_host = is_host.load(Ordering::Relaxed);
            // Рассылаем IrohSync через Iroh P2P для Full-Mesh связности
            if let Some(ref p2p) = self.network.iroh {
                let sync_msg = NetworkMessage::IrohSync {
                    sender_id: self.my_id,
                    node_id: p2p.my_node_id.clone(),
                    is_host: my_is_host,
                };
                let serialized = crate::network::message::serialize_msg(&sync_msg);
                p2p.broadcast_bytes(&serialized).await;
            }
            
            // Автоматическая очистка оффлайн-участников
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64;
            let dead_peers: Vec<u64> = self.room.get_all_peers().iter()
                .filter(|p| !p.is_host && now.saturating_sub(p.last_heartbeat) > 12000)
                .map(|p| p.id)
                .collect();
            for dead_id in dead_peers {
                println!("[СЕТЬ] Участник ID={} отключен по таймауту (12 сек молчания).", dead_id);
                self.room.remove_peer(dead_id);
            }
            
            let peers = self.room.get_all_peers();
            for peer in &peers {
                // Пропускаем Iroh-пиры для отправки по сырому UDP
                if peer.address.starts_with("Iroh:") {
                    continue;
                }
                match peer.address.parse::<std::net::SocketAddr>() {
                    Ok(addr) => {
                        let heartbeat_msg = NetworkMessage::Heartbeat {
                            sender_id: self.my_id,
                            is_host: my_is_host,
                        };
                        if let Err(e) = self.network.send_packet(addr, &heartbeat_msg).await {
                            eprintln!("[ОШИБКА] Не удалось отправить Heartbeat на {} (ID={}): {}", addr, peer.id, e);
                        }
                    }
                    Err(e) => {
                        eprintln!("[ОШИБКА] Некорректный адрес пира '{}' (ID={}): {}", peer.address, peer.id, e);
                    }
                }
            }
            
            // Если узел НЕ является хостом (!my_is_host), он дополнительно проверяет доступность текущего хоста
            if !my_is_host {
                let now = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64;
                
                // Получаем свежий список пиров перед проверкой доступности хоста
                let current_peers = self.room.get_all_peers();
                let host_opt = current_peers.iter().find(|p| p.is_host);
                if let Some(host) = host_opt {
                    // Если хост не отвечал более 3500мс, запускаем миграцию
                    if now.saturating_sub(host.last_heartbeat) > 3500 {
                        println!("[МИГРАЦИЯ] Хост ID={} не отвечает (дельта: {}мс). Начинаем выборы!", host.id, now.saturating_sub(host.last_heartbeat));
                        self.migrate_host(is_host.clone(), room_id.clone()).await;
                    }
                } else if !current_peers.is_empty() {
                    println!("[МИГРАЦИЯ] В комнате нет активного хоста. Инициируем выборы!");
                    self.migrate_host(is_host.clone(), room_id.clone()).await;
                }
            }
        }
    }

    async fn migrate_host(&self, is_host: Arc<AtomicBool>, room_id: String) {
        let peers = self.room.get_all_peers();
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        // Находим ID старого мертвого хоста
        let dead_host_id = peers.iter()
            .find(|peer| peer.is_host)
            .map(|peer| peer.id)
            .unwrap_or(0);
        
        // Немедленно удаляем мертвого хоста из комнаты, чтобы не плодить зомби
        if dead_host_id != 0 {
            println!("[МИГРАЦИЯ] Удаление отключившегося хоста ID={}", dead_host_id);
            self.room.remove_peer(dead_host_id);
        }

        // Собираем оставшихся живых кандидатов (не старше 8 секунд)
        let mut peer_ids: Vec<u64> = self.room.get_all_peers().iter()
            .filter(|peer| peer.id != dead_host_id && (now.saturating_sub(peer.last_heartbeat) < 8000))
            .map(|peer| peer.id)
            .collect();
        
        if !peer_ids.contains(&self.my_id) {
            peer_ids.push(self.my_id);
        }
        peer_ids.sort();
        
        if let Some(&next_host_id) = peer_ids.first() {
            if next_host_id == self.my_id {
                // Я победил на выборах и стал новым хостом
                is_host.store(true, Ordering::SeqCst);
                self.room.set_host(self.my_id);
                println!("[МИГРАЦИЯ] 👑 Я стал НОВЫМ ХОСТОМ (ID: {})!", self.my_id);
                
                let msg = NetworkMessage::HostMigration {
                    new_host_id: self.my_id,
                };
                let serialized = crate::network::message::serialize_msg(&msg);
                if let Some(ref p2p) = self.network.iroh {
                    p2p.broadcast_bytes(&serialized).await;
                }
            } else {
                // Победил сосед с меньшим ID, признаем его новым хостом
                println!("[МИГРАЦИЯ] Признан новый лидер ID: {}. Переключаю статус хоста.", next_host_id);
                self.room.set_host(next_host_id);
                is_host.store(false, Ordering::SeqCst);
            }
        }
    }
}