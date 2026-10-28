use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::net::SocketAddr;
use discordrf::{
    room::RoomManager, mixer::AudioMixer, audio::UniversalAudioCapture, 
    network::NetworkManager, migration::MigrationManager, gui::RadioApp, 
    devices::DeviceSelector
};

fn main() {
    // 1. Автоматическая настройка правил фаервола Windows
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
    let is_host = Arc::new(AtomicBool::new(true));
    let rt = tokio::runtime::Runtime::new().unwrap();
    
    let selected_input = Arc::new(AtomicU32::new(0));
    let selected_output = Arc::new(AtomicU32::new(0));
    let ext_ip = Arc::new(Mutex::new("Определение...".to_string()));
    let mic_stream = Arc::new(Mutex::new(None));

    // 2. Инициализация Iroh P2P
    let iroh_p2p = rt.block_on(async {
        match discordrf::network::p2p_iroh::IrohP2pManager::new().await {
            Ok(p2p) => {
                println!("[IROH] Эндпоинт готов. NodeID: {}", p2p.my_node_id);
                Some(Arc::new(p2p))
            }
            Err(e) => {
                eprintln!("[IROH] Ошибка инициализации P2P: {}", e);
                None
            }
        }
    });

    if let Some(ref p2p) = iroh_p2p {
        if let Ok(mut lock) = ext_ip.lock() {
            *lock = p2p.my_node_id.clone();
        }
    }
    
    // 3. Сетевой сокет UDP + UPnP + STUN
    let std_socket = std::net::UdpSocket::bind(socket_addr).unwrap_or_else(|e| {
        panic!("Не удалось привязать сокет к {}: {}", socket_addr, e);
    });
    
    if let Ok(local_addr) = std_socket.local_addr() {
        discordrf::upnp::auto_forward_port(local_addr.port());
    }
    
    println!("[STUN] Определение внешнего UDP адреса для P2P...");
    let stun_addr = discordrf::stun::get_external_addr(&std_socket);
    let stun_resolved = stun_addr.is_some();
    if let Some(ext_addr) = stun_addr {
        println!("[STUN] Внешний UDP адрес для P2P: {}", ext_addr);
        if iroh_p2p.is_none() {
            if let Ok(mut lock) = ext_ip.lock() {
                *lock = ext_addr.to_string();
            }
        }
    } else {
        println!("[STUN] Не удалось определить адрес через STUN, используется локальный");
    }

    let network = rt.block_on(async {
        Arc::new(
            NetworkManager::from_std(std_socket, iroh_p2p.clone())
                .expect("Не удалось инициализировать NetworkManager из сокета")
        )
    });
    
    if !stun_resolved && iroh_p2p.is_none() {
        let ext_ip_clone = ext_ip.clone();
        rt.spawn(async move {
            let ip = discordrf::ip_detector::get_external_ip();
            if let Ok(mut lock) = ext_ip_clone.lock() {
                *lock = ip;
            }
        });
    }
    
    let room = Arc::new(RoomManager::new(my_id));
    let mixer = Arc::new(AudioMixer::new());
    let mut audio = UniversalAudioCapture::new().unwrap();
    let migration = Arc::new(MigrationManager::new(my_id, room.clone(), network.clone()));

    // 4. Каналы передачи аудио
    let (packet_tx, packet_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);
    let (playback_tx, playback_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);
    let playback_rx = Arc::new(std::sync::Mutex::new(playback_rx));
    let playback_stream = Arc::new(Mutex::new(None));
    
    if let Ok(stream) = discordrf::audio::UniversalAudioCapture::create_playback_stream(None, playback_rx.clone()) {
        *playback_stream.lock().unwrap() = Some(stream);
    }
    
    if let Ok(stream) = audio.create_input_stream(None, ptt_flag.clone(), packet_tx.clone(), mixer.clone()) {
        *mic_stream.lock().unwrap() = Some(stream);
    }

    // 5. Запуск фоновых сервисов (декомпозированные модули)
    discordrf::network::receiver::spawn_network_receiver(&rt, network.clone(), room.clone(), playback_tx.clone());
    discordrf::network::iroh_service::spawn_iroh_receiver(&rt, iroh_p2p.clone(), room.clone(), playback_tx.clone(), is_host.clone());
    discordrf::network::broadcast::spawn_audio_broadcast(&rt, packet_rx, room.clone(), network.clone(), my_id);
    
    let migration_loop = migration.clone();
    let is_host_loop = is_host.clone();
    let room_id_str = my_id.to_string();
    rt.spawn(async move {
        println!("[МИГРАЦИЯ] Фоновый цикл Heartbeat/Failover запущен.");
        migration_loop.start_heartbeat_loop(is_host_loop, room_id_str).await;
    });

    // 6. Запуск графического интерфейса
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