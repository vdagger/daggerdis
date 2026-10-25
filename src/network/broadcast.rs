use std::sync::Arc;
use tokio::sync::mpsc::Receiver;

pub fn spawn_audio_broadcast(
    rt: &tokio::runtime::Runtime,
    mut packet_rx: Receiver<Vec<u8>>,
    room: Arc<crate::room::RoomManager>,
    network: Arc<crate::network::NetworkManager>,
) {
    let room_net = room.clone();
    rt.spawn(async move {
        println!("[СЕТЬ] Готов к трансляции аудиопакетов в P2P...");
        while let Some(packet) = packet_rx.recv().await {
            let network_sender = network.clone();
            let my_id = room_net.get_my_id();
            let msg = crate::network::message::NetworkMessage::AudioFrame {
                sender_id: my_id,
                opus_data: packet,
            };
            let serialized_bytes = crate::network::message::serialize_msg(&msg);
            let peers = room_net.get_all_peers();
            for peer in peers {
                if peer.id == my_id {
                    continue;
                }
                if let Ok(addr) = peer.address.parse::<std::net::SocketAddr>() {
                    let _ = network_sender.send_audio_packet(addr, &serialized_bytes).await;
                }
            }
        }
    });
}