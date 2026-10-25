use std::sync::Arc;
use tokio::sync::mpsc::Receiver;

pub fn spawn_audio_broadcast(
    rt: &tokio::runtime::Runtime,
    mut packet_rx: Receiver<Vec<u8>>,
    room: Arc<crate::room::RoomManager>,
    _network: Arc<crate::network::NetworkManager>,
) {
    let room_net = room.clone();
    rt.spawn(async move {
        println!("[СЕТЬ] Готов к трансляции аудиопакетов в P2P...");
        while let Some(packet) = packet_rx.recv().await {
            let peers = room_net.get_all_peers(); 
            for peer in peers {
                // Вызови метод отправки пакета пиру в соответствии с сигнатурой твоего NetworkManager
                // Например: let _ = network_net.send_packet(&peer.address, &packet).await;
            }
        }
    });
}