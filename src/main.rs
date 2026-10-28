use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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
    let is_host = Arc::new(AtomicBool::new(true)); // Объявлен в самом начале!
    let rt = tokio::runtime::Runtime::new().unwrap();
    
    let selected_input = Arc::new(AtomicU32::new(0));
    let selected_output = Arc::new(AtomicU32::new(0));
    let ext_ip = Arc::new(Mutex::new("Определение...".to_string()));
    let mic_stream = Arc::new(Mutex::new(None));

    // 1. Инициализация Iroh P2P через рантайм Tokio
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

    // Если Iroh запустился, устанавливаем NodeID как основной адрес комнаты
    if let Some(ref p2p) = iroh_p2p {
        if let Ok(mut lock) = ext_ip.lock() {
            *lock = p2p.my_node_id.clone();
        }
    }
    
    // 2. Связываем стандартный UDP-сокет один раз
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
        if iroh_p2p.is_none() {
            if let Ok(mut lock) = ext_ip.lock() {
                *lock = ext_addr.to_string();
            }
        }
    } else {
        println!("[STUN] Не удалось определить адрес через STUN, используется локальный");
    }

    // Запускаем сетевой менеджер (передаем сокет и Iroh)
    let network = rt.block_on(async {
        Arc::new(
            NetworkManager::from_std(std_socket, iroh_p2p.clone())
                .expect("Не удалось инициализировать NetworkManager из сокета")
        )
    });
    
    // Запускаем определение внешнего IP только если STUN не дал результата и Iroh нет
    if !stun_resolved && iroh_p2p.is_none() {
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
    
    // UDP-приемник
    discordrf::network::receiver::spawn_network_receiver(&rt, network.clone(), room.clone(), playback_tx.clone());

    // 3. Приемник входящих P2P пакетов через Iroh + Ретранслятор (Relay) для Хоста
    if let Some(ref p2p) = iroh_p2p {
        let p2p_recv = p2p.clone();
        let playback_tx_iroh = playback_tx.clone();
        let room_iroh = room.clone();
        let is_host_iroh = is_host.clone(); // Клонируем ДО rt.spawn!

        rt.spawn(async move {
            println!("[IROH] Слушатель входящих P2P пакетов активен.");
            while let Some(pkt) = p2p_recv.recv_packet().await {
                if let Some(msg) = discordrf::network::message::deserialize_msg(&pkt.data) {
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

                            room_iroh.update_peer(discordrf::types::Peer {
                                id: sender_id,
                                address: format!("Iroh: {}..", short_id),
                                is_host: is_peer_host,
                                last_heartbeat: now,
                            });

                            let _ = playback_tx_iroh.try_send(opus_data);

                            // Ретрансляция голоса: если мы Хост, пересылаем всем остальным
                            if is_host_iroh.load(Ordering::Relaxed) {
                                p2p_recv.broadcast_bytes_except(&pkt.sender_id, &pkt.data).await;
                            }
                        }
                        NetworkMessage::Heartbeat { sender_id, is_host: peer_is_host } => {
                            room_iroh.update_peer(discordrf::types::Peer {
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

    if let Ok(stream) = audio.create_input_stream(None, ptt_flag.clone(), packet_tx.clone(), mixer.clone()) {
        *mic_stream.lock().unwrap() = Some(stream);
    }
    
    // 4. Отправка голоса с микрофона
    let network_net = network.clone();
    let room_net = room.clone();
    rt.spawn(async move {
        println!("[СЕТЬ] Готов к трансляции аудиопакетов in P2P...");
        let mut send_count = 0u64;
        while let Some(packet) = packet_rx.recv().await {
            // Трансляция через Iroh P2P
            if let Some(ref p2p) = network_net.iroh {
                let network_message = NetworkMessage::AudioFrame {
                    sender_id: my_id,
                    opus_data: packet.clone(),
                };
                let serialized = discordrf::network::message::serialize_msg(&network_message);
                p2p.broadcast_bytes(&serialized).await;
            }

            // Трансляция через сырой UDP (пропуская Iroh-пиров)
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

    // 5. Фоновый цикл миграции и heartbeat
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