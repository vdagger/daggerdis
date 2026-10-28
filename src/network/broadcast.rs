//! Фоновый сервис трансляции аудиопакетов с микрофона в P2P-сеть

use std::sync::Arc;
use std::net::SocketAddr;
use tokio::sync::mpsc::Receiver;
use crate::network::NetworkMessage;
use crate::room::RoomManager;
use crate::network::NetworkManager;

/// Запускает фоновую таску чтения пакетов микрофона и их отправки в Iroh и UDP
pub fn spawn_audio_broadcast(
    rt: &tokio::runtime::Runtime,
    mut packet_rx: Receiver<Vec<u8>>,
    room: Arc<RoomManager>,
    network: Arc<NetworkManager>,
    my_id: u64,
) {
    let network_net = network.clone();
    let room_net = room.clone();
    
    rt.spawn(async move {
        println!("[СЕТЬ] Готов к трансляции аудиопакетов in P2P...");
        let mut send_count = 0u64;
        while let Some(packet) = packet_rx.recv().await {
            // 1. Трансляция через Iroh P2P
            if let Some(ref p2p) = network_net.iroh {
                let network_message = NetworkMessage::AudioFrame {
                    sender_id: my_id,
                    opus_data: packet.clone(),
                };
                let serialized = crate::network::message::serialize_msg(&network_message);
                p2p.broadcast_bytes(&serialized).await;
            }

            // 2. Трансляция через сырой UDP (пропуская Iroh-пиров)
            let peers = room_net.get_all_peers(); 
            for peer in peers {
                if peer.address.starts_with("Iroh:") {
                    continue;
                }
                let network_message = NetworkMessage::AudioFrame {
                    sender_id: my_id,
                    opus_data: packet.clone(),
                };
                if let Ok(addr) = peer.address.parse::<SocketAddr>() {
                    let _ = network_net.send_packet(addr, &network_message).await;
                }
            }
            send_count += 1;
            if send_count % 100 == 1 {
                println!("[АУДИО] Отправка голоса в P2P (пакет #{}, размер: {} байт)", send_count, packet.len());
            }
        }
    });
}