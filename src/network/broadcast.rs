//! Фоновая трансляция голосовых пакетов через Iroh P2P

use crate::network::p2p_iroh::IrohP2pManager;
use crate::network::NetworkMessage;
use std::sync::Arc;
use tokio::sync::mpsc::Receiver;

/// Рассылает голос с микрофона всем активным Iroh-соседям.
pub fn spawn_audio_broadcast(
    rt: &tokio::runtime::Runtime,
    mut packet_rx: Receiver<Vec<u8>>,
    iroh: Option<Arc<IrohP2pManager>>,
    my_id: u64,
) {
    if let Some(p2p) = iroh {
        rt.spawn(async move {
            let mut seq: u32 = 0;
            while let Some(packet) = packet_rx.recv().await {
                let msg = NetworkMessage::AudioFrame {
                    sender_id: my_id,
                    seq,
                    opus_data: packet,
                };
                let bytes = crate::network::message::serialize_msg(&msg);
                p2p.broadcast_bytes(&bytes).await;
                seq = seq.wrapping_add(1);
            }
        });
    }
}
