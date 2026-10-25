use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;
use std::net::SocketAddr;
use discordrf::{
    room::RoomManager, mixer::AudioMixer, audio::UniversalAudioCapture, 
    network::NetworkManager, migration::MigrationManager, gui::RadioApp, protocol::{Packet, NetworkMessage}
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let local_addr = if args.len() >= 2 { format!("0.0.0.0:{}", args[1]) } else { "0.0.0.0:8080".to_string() };
    
    // Преобразуем строку в SocketAddr
    let socket_addr: SocketAddr = local_addr.parse().unwrap();
    
    let my_id: u64 = rand::random();
    let ptt_flag = Arc::new(AtomicBool::new(false));
    let rt = tokio::runtime::Runtime::new().unwrap();
    
    let selected_input = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let selected_output = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let ext_ip = Arc::new(Mutex::new("Определение...".to_string()));
    let mic_stream = Arc::new(Mutex::new(None));
    
    // Запускаем асинхронную задачу определения IP
    let ext_ip_clone = ext_ip.clone();
    rt.spawn(async move {
        println!("[СЕТЬ] Запуск определения внешнего IP...");
        // Используем синхронную версию, так как get_external_ip() не асинхронная
        let ip = discordrf::ip_detector::get_external_ip();
        println!("[СЕТЬ] IP определен: {}", ip);
        if let Ok(mut lock) = ext_ip_clone.lock() {
            *lock = ip;
        }
    });
    
    let network = rt.block_on(async { Arc::new(NetworkManager::new(socket_addr).await.unwrap()) });
    let room = Arc::new(RoomManager::new(my_id));
    let mixer = Arc::new(AudioMixer::new());
    let mut audio = UniversalAudioCapture::new().unwrap();
    let _migration = Arc::new(MigrationManager::new(my_id, room.clone(), network.clone()));
    
    let (packet_tx, mut packet_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);
    
    if let Ok(stream) = audio.create_input_stream(ptt_flag.clone(), packet_tx.clone(), mixer.clone()) {
        *mic_stream.lock().unwrap() = Some(stream);
    }
    
    // Запускаем асинхронную задачу трансляции аудиопакетов в P2P
    let network_net = network.clone();
    let room_net = room.clone();
    rt.spawn(async move {
        println!("[СЕТЬ] Готов к трансляции аудиопакетов в P2P...");
        while let Some(packet) = packet_rx.recv().await {
            // Получаем список адресов всех пиров в текущей комнате рации
            let peers = room_net.get_all_peers(); 
            for peer in peers {
                // Создаем NetworkMessage с аудиоданными
                let network_message = NetworkMessage {
                    room_id: my_id.to_string(), // Используем ID пользователя как ID комнаты
                    sender_id: my_id,
                    packet: Packet::AudioData(packet.clone()), // Клонируем пакет для использования в цикле
                };
                // Преобразуем адрес из String в SocketAddr для отправки
                if let Ok(addr) = peer.address.parse::<SocketAddr>() {
                    // Отправляем сжатый Opus-пакет по UDP/P2P через NetworkManager
                    let _ = network_net.send_packet(addr, &network_message).await;
                }
            }
        }
    // Запускаем асинхронную задачу приема входящих аудиопакетов
    let (playback_tx, _playback_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);
    discordrf::network::receiver::spawn_network_receiver(&rt, network.clone(), room.clone(), playback_tx);
        "Discord-Рация Rust 2026",
        eframe::NativeOptions::default(),
        Box::new(move |_cc| {
            Ok(Box::new(RadioApp::new(
                mixer, room, None, ptt_flag, selected_input, selected_output,
                network, my_id, ext_ip, mic_stream
            )))
        }),
    ).unwrap();
}