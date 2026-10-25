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
                    if let Ok(addr) = peer.address.parse() {
                        let _ = self.network.send_packet(addr, &heartbeat_msg).await;
                    }
                }
            } else {
                // Клиент проверяет последний полученный heartbeat от хоста
                let peers = self.room.get_all_peers();
                let now = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as u64;
                
                // Ищем хоста и проверяем его последний heartbeat
                let host_opt = peers.iter().find(|p| p.is_host);
                if let Some(host) = host_opt {
                    if now - host.last_heartbeat > 2000 {
                        // Хост не отвечает более 2 секунд, запускаем миграцию
                        println!("[МИГРАЦИЯ] Хост ID={} не отвечает более 2 сек (дельта: {}мс). Начинаем выборы!", host.id, now - host.last_heartbeat);
                        self.migrate_host(is_host.clone(), room_id.clone()).await;
                    }
                } else if !peers.is_empty() {
                    // Комната осталась без лидера
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
        
        // Находим ID старого хоста
        let dead_host_id = peers.iter()
            .find(|peer| peer.is_host)
            .map(|peer| peer.id)
            .unwrap_or(0);
        
        // Собираем список кандидатов, исключая старого мертвого хоста и тех, кто не выходил на связь более 4 секунд
        let mut peer_ids: Vec<u64> = peers.iter()
            .filter(|peer| peer.id != dead_host_id && (now.saturating_sub(peer.last_heartbeat) < 4000))
            .map(|peer| peer.id)
            .collect();
        
        println!("[МИГРАЦИЯ] Кандидаты на роль хоста: {:?}, мой ID: {}", peer_ids, self.my_id);
        
        // Обязательно убедиться, что self.my_id присутствует в списке кандидатов
        if !peer_ids.contains(&self.my_id) {
            peer_ids.push(self.my_id);
        }
        
        // Сортируем список оставшихся ID по возрастанию
        peer_ids.sort();
        
        // Находим следующий по приоритету ID (минимальный)
        if let Some(next_host_id) = peer_ids.first() {
            if *next_host_id == self.my_id {
                // Становимся новым хостом
                is_host.store(true, Ordering::SeqCst);
                println!("[СЕТЬ] Старый хост (ID: {}) отключен. Я стал НОВЫМ ХОСТОМ!", dead_host_id);
                
                // Если победили на выборах и стали хостом:
                // 1) Вызвать self.room.set_host(self.my_id);
                self.room.set_host(self.my_id);
                
                // 2) Если был определен dead_host_id, удалить старого мертвого хоста
                if dead_host_id != 0 {
                    self.room.remove_peer(dead_host_id);
                }
                
                // 3) Разослать сообщение Packet::MigrateHost(self.my_id) всем оставшимся пирам
                let msg = NetworkMessage {
                    room_id: room_id.clone(),
                    sender_id: self.my_id,
                    packet: Packet::MigrateHost(self.my_id),
                };
                for peer in self.room.get_all_peers() {
                    if let Ok(addr) = peer.address.parse() {
                        let _ = self.network.send_packet(addr, &msg).await;
                    }
                }
            } else {
                println!("[МИГРАЦИЯ] Выбран кандидат с меньшим ID: {}. Ожидаю от него статус хоста...", next_host_id);
            }
        }
    }
}