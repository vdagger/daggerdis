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
                    }
                    crate::network::message::NetworkMessage::AudioFrame { sender_id: _, opus_data } => {
                        let _ = playback_tx_clone.send(opus_data).await;
                    }
                    crate::network::message::NetworkMessage::HostMigration { new_host_id } => {
                        println!("[СЕТЬ] Получено уведомление о смене хоста. Новый хост ID: {}", new_host_id);
                        room_net.set_host(new_host_id);
                    }
                    _ => {}
                }
            }
        }
    });
}