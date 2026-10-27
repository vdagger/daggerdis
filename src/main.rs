use std::sync::{Arc, Mutex};
use std::sync::atomic::AtomicBool;
use std::net::SocketAddr;
use discordrf::{
    room::RoomManager, mixer::AudioMixer, audio::UniversalAudioCapture, 
    network::NetworkManager, migration::MigrationManager, gui::RadioApp, 
    network::NetworkMessage,
    devices::DeviceSelector
};

fn main() {
    // Настройка правил фаервола Windows
    discordrf::firewall::setup_firewall_rules();
    
    let args: Vec<String> = std::env::args().collect();
    let local_addr = if args.len() >= 2 { 
        format!("0.0.0.0:{}", args[1]) 
    } else { 
        "0.0.0.0:0".to_string() 
    };
    
    let socket_addr: SocketAddr = local_addr.parse().unwrap();
    let my_id: u64 = rand::random();
    let ptt_flag = Arc::new(AtomicBool::new(false));
    let rt = tokio::runtime::Runtime::new().unwrap();
    
    let selected_input = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let selected_output = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let ext_ip = Arc::new(Mutex::new("Определение...".to_string()));
    let mic_stream = Arc::new(Mutex::new(None));
    
    // Связываем стандартный UDP-сокет один раз
    let std_socket = std::net::UdpSocket::bind(socket_addr).unwrap_or_else(|e| {
        panic!("Не удалось привязать сокет к {}: {}", socket_addr, e);
    });
    
    // Получаем локальный порт и пробрасываем его через UPnP
    if let Ok(local_addr) = std_socket.local_addr() {
        discordrf::upnp::auto_forward_port(local_addr.port());
    }
    
    // Выполняем STUN-запрос через этот же сокет ДО перевода в nonblocking
    println!("[STUN] Определение внешнего UDP адреса для P2P...");
    let stun_addr = discordrf::stun::get_external_addr(&std_socket);
    let stun_resolved = stun_addr.is_some();
    if let Some(ext_addr) = stun_addr {
        println!("[STUN] Внешний UDP адрес для P2P: {}", ext_addr);
        if let Ok(mut lock) = ext_ip.lock() {
            *lock = ext_addr.to_string();
        }
    } else {
        println!("[STUN] Не удалось определить адрес через STUN, используется локальный");
    }
    
    // Запускаем сетевой менеджер с уже привязанным и сконфигурированным сокетом
    let network = rt.block_on(async {
        Arc::new(
            NetworkManager::from_std(std_socket)
                .expect("Не удалось инициализировать NetworkManager из сокета")
        )
    });
    
    // Запускаем определение внешнего IP только если STUN не дал результата
    if !stun_resolved {
        let ext_ip_clone = ext_ip.clone();
        rt.spawn(async move {
            println!("[СЕТЬ] Запуск определения внешнего IP...");
            let ip = discordrf::ip_detector::get_external_ip();
            println!("[СЕТЬ] IP определен: {}", ip);
            if let Ok(mut lock) = ext_ip_clone.lock() {
                *lock = ip;
            }
        });
    }
    
    let room = Arc::new(RoomManager::new(my_id));
    let mixer = Arc::new(AudioMixer::new());
    let mut audio = UniversalAudioCapture::new().unwrap();
    let migration = Arc::new(MigrationManager::new(my_id, room.clone(), network.clone()));
    
    let (packet_tx, mut packet_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);
    
    // Инициализация потока воспроизведения
    let (playback_tx, playback_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);
    let playback_rx = Arc::new(std::sync::Mutex::new(playback_rx));
    let playback_stream = Arc::new(Mutex::new(None));
    
    // Запускаем стартовый поток воспроизведения
    if let Ok(stream) = discordrf::audio::UniversalAudioCapture::create_playback_stream(None, playback_rx.clone()) {
        *playback_stream.lock().unwrap() = Some(stream);
    }
    
    discordrf::network::receiver::spawn_network_receiver(&rt, network.clone(), room.clone(), playback_tx);
    
    if let Ok(stream) = audio.create_input_stream(None, ptt_flag.clone(), packet_tx.clone(), mixer.clone()) {
        *mic_stream.lock().unwrap() = Some(stream);
    }
    
    let network_net = network.clone();
    let room_net = room.clone();
    rt.spawn(async move {
        println!("[СЕТЬ] Готов к трансляции аудиопакетов in P2P...");
        let mut send_count = 0u64;
        while let Some(packet) = packet_rx.recv().await {
            let peers = room_net.get_all_peers(); 
            for peer in peers {
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
    
    // Создаем флаг хоста
    let is_host = Arc::new(AtomicBool::new(true));
    
    // Запускаем фоновый цикл миграции и heartbeat
    let migration_loop = migration.clone();
    let is_host_loop = is_host.clone();
    let room_id_str = my_id.to_string();
    rt.spawn(async move {
        println!("[МИГРАЦИЯ] Фоновый цикл Heartbeat/Failover запущен.");
        migration_loop.start_heartbeat_loop(is_host_loop, room_id_str).await;
    });

    let input_devices = DeviceSelector::get_input_devices();
    let output_devices = DeviceSelector::get_output_devices();

    let _guard = rt.enter();
    let _runtime_guard = rt;
    eframe::run_native(
        "Discord-Рация Rust 2026",
        eframe::NativeOptions::default(),
        Box::new(move |_cc| {
            Ok(Box::new(RadioApp::new(
                mixer, room, None, ptt_flag, selected_input, selected_output,
                network, my_id, ext_ip, mic_stream, packet_tx, input_devices, output_devices, is_host,
                playback_stream, playback_rx
            )))
        }),
    ).unwrap();
}