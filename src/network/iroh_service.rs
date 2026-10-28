//! Фоновый сервис приема и ретрансляции пакетов через Iroh P2P

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::mpsc::Sender;
use crate::network::p2p_iroh::IrohP2pManager;
use crate::room::RoomManager;
use crate::network::NetworkMessage;

/// Запускает фоновую таску приема аудио и Heartbeat через Iroh, включая ретрансляцию (SFU) на хосте
pub fn spawn_iroh_receiver(
    rt: &tokio::runtime::Runtime,
    iroh_p2p: Option<Arc<IrohP2pManager>>,
    room: Arc<RoomManager>,
    playback_tx: Sender<Vec<u8>>,
    is_host: Arc<AtomicBool>,
) {
    if let Some(p2p) = iroh_p2p {
        let p2p_recv = p2p.clone();
        let playback_tx_iroh = playback_tx.clone();
        let room_iroh = room.clone();
        let is_host_iroh = is_host.clone();

        rt.spawn(async move {
            println!("[IROH] Слушатель входящих P2P пакетов активен.");
            while let Some(pkt) = p2p_recv.recv_packet().await {
                if let Some(msg) = crate::network::message::deserialize_msg(&pkt.data) {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64;

                    let short_id = if pkt.sender_id.len() > 10 {
                        &pkt.sender_id[..10]
                    } else {
                        &pkt.sender_id
                    };

                    match msg {
                        NetworkMessage::AudioFrame { sender_id, opus_data } => {
                            let is_peer_host = room_iroh.get_all_peers().iter()
                                .find(|p| p.id == sender_id)
                                .map(|p| p.is_host)
                                .unwrap_or(false);

                            room_iroh.update_peer(crate::types::Peer {
                                id: sender_id,
                                address: format!("Iroh: {}..", short_id),
                                is_host: is_peer_host,
                                last_heartbeat: now,
                            });

                            let _ = playback_tx_iroh.try_send(opus_data);

                            // Ретрансляция голоса: если мы Хост, пересылаем всем остальным участникам
                            if is_host_iroh.load(Ordering::Relaxed) {
                                p2p_recv.broadcast_bytes_except(&pkt.sender_id, &pkt.data).await;
                            }
                        }
                        NetworkMessage::Heartbeat { sender_id, is_host: peer_is_host } => {
                            room_iroh.update_peer(crate::types::Peer {
                                id: sender_id,
                                address: format!("Iroh: {}..", short_id),
                                is_host: peer_is_host,
                                last_heartbeat: now,
                            });

                            // Ретрансляция Heartbeat: чтобы клиенты видели друг друга
                            if is_host_iroh.load(Ordering::Relaxed) {
                                p2p_recv.broadcast_bytes_except(&pkt.sender_id, &pkt.data).await;
                            }
                        }
                        _ => {}
                    }
                }
            }
        });
    }
}