use std::sync::Arc;
use tokio::sync::mpsc::Sender;
use crate::network::NetworkManager;
use crate::room::RoomManager;

pub fn spawn_network_receiver(
    rt: &tokio::runtime::Runtime,
    network: Arc<NetworkManager>,
    room: Arc<RoomManager>,
    playback_tx: Sender<Vec<u8>>,
) {
    let network_recv = network.clone();
    let room_net = room.clone();
    let playback_tx_clone = playback_tx.clone();
    
    rt.spawn(async move {
        println!("[СЕТЬ] Асинхронный UDP-приемник Mesh-сети запущен.");
        // Выделяем буфер локально внутри таски, чтобы ссылки не пересекали .await точки
        let mut buf = [0u8; 65535];
        while let Ok((amt, sender_addr)) = network_recv.recv_audio_packet(&mut buf).await {
            let bytes = &buf[..amt];
            if let Some(msg) = crate::network::message::deserialize_msg(bytes) {
                match msg {
                    crate::network::message::NetworkMessage::Heartbeat { sender_id, is_host } => {
                        if is_host {
                            let peer = crate::Peer {
                                id: sender_id,
                                address: sender_addr.to_string(),
                                is_host,
                                last_heartbeat: std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_millis() as u64,
                            };
                            room_net.update_peer(peer);
                        }
                        println!("[СЕТЬ] Получен Heartbeat от хоста ID={}", sender_id);
                    }
                    crate::network::message::NetworkMessage::AudioFrame { sender_id: _, opus_data } => {
                        let _ = playback_tx_clone.send(opus_data).await;
                    }
                    crate::network::message::NetworkMessage::HostMigration { new_host_id } => {
                        println!("[СЕТЬ] Получено уведомление о смене хоста. Новый хост ID: {}", new_host_id);
                        room_net.set_host(new_host_id);
                    }
                    crate::network::message::NetworkMessage::JoinRequest { sender_id } => {
                        println!("[СЕТЬ] Получен JoinRequest от ID={} с адреса {}", sender_id, sender_addr);
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;

                        // 1. Добавляем новичка в список участников
                        let new_peer = crate::types::Peer {
                            id: sender_id,
                            address: sender_addr.to_string(),
                            is_host: false,
                            last_heartbeat: now,
                        };
                        room_net.update_peer(new_peer);

                        // 2. Формируем актуальный список всех пиров для рассылки
                        let mut peer_tuples: Vec<(u64, std::net::SocketAddr)> = Vec::new();
                        for p in room_net.get_all_peers() {
                            if let Ok(addr) = p.address.parse::<std::net::SocketAddr>() {
                                peer_tuples.push((p.id, addr));
                            }
                        }
                        // Добавляем текущий узел (хоста)
                        let my_id = room_net.get_my_id();
                        if let Ok(my_addr) = network_recv.local_addr() {
                            peer_tuples.push((my_id, my_addr));
                        }

                        // 3. Отправляем обновленный список новичку и всем участникам
                        let update_msg = crate::network::message::NetworkMessage::PeerListUpdate {
                            peers: peer_tuples,
                        };
                        let serialized = crate::network::message::serialize_msg(&update_msg);
                        
                        // Отправляем новичку
                        let _ = network_recv.send_audio_packet(sender_addr, &serialized).await;

                        // Оповещаем остальных пиров
                        for p in room_net.get_all_peers() {
                            if p.id != sender_id {
                                if let Ok(addr) = p.address.parse::<std::net::SocketAddr>() {
                                    let _ = network_recv.send_audio_packet(addr, &serialized).await;
                                }
                            }
                        }
                    }
                    crate::network::message::NetworkMessage::PeerListUpdate { peers } => {
                        println!("[СЕТЬ] Получен список участников сети ({} пиров)", peers.len());
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        let my_id = room_net.get_my_id();

                        for (peer_id, mut peer_addr) in peers {
                            if peer_id == my_id {
                                continue;
                            }
                            // Если адрес пришел как 0.0.0.0, подставляем IP отправителя
                            if peer_addr.ip().is_unspecified() {
                                peer_addr.set_ip(sender_addr.ip());
                            }
                            let peer = crate::types::Peer {
                                id: peer_id,
                                address: peer_addr.to_string(),
                                is_host: false, // статус хоста уточнится первым же Heartbeat
                                last_heartbeat: now,
                            };
                            room_net.update_peer(peer);
                        }
                        
                        // Устанавливаем текущего хоста на основе адреса отправителя
                        for peer in room_net.get_all_peers() {
                            if peer.address.contains(&sender_addr.to_string()) || 
                               peer.address.contains(&format!(":{}", sender_addr.port())) {
                                let mut updated_peer = peer.clone();
                                updated_peer.is_host = true;
                                room_net.update_peer(updated_peer);
                                break;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    });
}