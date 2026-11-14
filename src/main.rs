use albus::{
    audio::UniversalAudioCapture, devices::DeviceSelector, gui::RadioApp,
    migration::MigrationManager, mixer::AudioMixer, network::p2p_iroh::IrohP2pManager,
    room::RoomManager,
};
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, Mutex};

fn main() {
    // 1. Автоматическая настройка правил фаервола Windows (входящий UDP для iroh QUIC)
    albus::firewall::setup_firewall_rules();

    // Закреплённая личность: секретный ключ iroh (NodeID) и стабильный псевдоним my_id
    let identity = albus::identity::Identity::load_or_create();
    let my_id: u64 = identity.my_id;
    let my_node_id: String = identity.node_id.clone();

    let ptt_flag = Arc::new(AtomicBool::new(false));
    let is_host = Arc::new(AtomicBool::new(true));
    let rt = tokio::runtime::Runtime::new().unwrap();

    // Загруженные настройки: устройства ищутся по имени, порядок индексов
    // между запусками не гарантирован
    let config = albus::config::AppConfig::load();
    let friends = Arc::new(albus::friends::FriendsManager::load());
    let input_devices = DeviceSelector::get_input_devices();
    let input_channel_counts = DeviceSelector::get_input_channel_counts();
    let output_devices = DeviceSelector::get_output_devices();

    let input_index = config
        .input_device
        .as_deref()
        .and_then(|name| input_devices.iter().position(|d| d == name))
        .or_else(DeviceSelector::default_input_index)
        .unwrap_or(0) as u32;
    let output_index = config
        .output_device
        .as_deref()
        .and_then(|name| output_devices.iter().position(|d| d == name))
        .or_else(DeviceSelector::default_output_index)
        .unwrap_or(0) as u32;

    let selected_input = Arc::new(AtomicU32::new(input_index));
    let selected_output = Arc::new(AtomicU32::new(output_index));
    let mic_stream = Arc::new(Mutex::new(None));

    // 2. Инициализация Iroh P2P с закреплённым секретным ключом.
    // При сбое приложение продолжает работать без сети и показывает ошибку в UI.
    let (iroh_p2p, network_error) = rt.block_on(async {
        match IrohP2pManager::new(identity.secret_key).await {
            Ok(p2p) => {
                println!("[IROH] Эндпоинт готов. NodeID: {}", p2p.my_node_id);
                (Some(Arc::new(p2p)), None)
            }
            Err(e) => {
                let msg = format!("Сеть не запущена: {}", e);
                eprintln!("[IROH] {}", msg);
                (None, Some(msg))
            }
        }
    });

    let room = Arc::new(RoomManager::new(my_id));
    let mixer = Arc::new(AudioMixer::new());
    if let Some(v) = config.ch1_volume {
        mixer.set_ch1_volume(v);
    }
    if let Some(v) = config.ch2_volume {
        mixer.set_ch2_volume(v);
    }
    if let Some(v) = config.output_volume {
        mixer.set_output_volume(v);
    }
    if let Some(c) = config.input1_channel {
        mixer.set_input1_channel(c);
    }
    if let Some(c) = config.input2_channel {
        mixer.set_input2_channel(c);
    }
    for (id, vol) in &config.peer_volumes {
        mixer.set_peer_volume(*id, *vol);
    }
    if let Some(vk) = config.ptt_vk {
        albus::keybind::CURRENT_VK.store(vk, std::sync::atomic::Ordering::Relaxed);
    }
    let mut audio = UniversalAudioCapture::new().unwrap();
    let migration = Arc::new(MigrationManager::new(my_id, room.clone(), iroh_p2p.clone()));

    // 3. Каналы передачи аудио
    let (packet_tx, packet_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);
    let (playback_tx, playback_rx) = tokio::sync::mpsc::channel::<(u64, u32, Vec<u8>)>(100);
    let playback_rx = Arc::new(std::sync::Mutex::new(playback_rx));
    let playback_stream = Arc::new(Mutex::new(None));

    let configured_output = config
        .output_device
        .as_deref()
        .and_then(DeviceSelector::find_output_device_by_name);
    if let Ok(stream) = albus::audio::UniversalAudioCapture::create_playback_stream(
        configured_output,
        playback_rx.clone(),
        mixer.clone(),
    ) {
        *playback_stream.lock().unwrap() = Some(stream);
    }

    let configured_input = config
        .input_device
        .as_deref()
        .and_then(DeviceSelector::find_input_device_by_name);
    if let Ok(stream) =
        audio.create_input_stream(configured_input, ptt_flag.clone(), packet_tx.clone(), mixer.clone())
    {
        *mic_stream.lock().unwrap() = Some(stream);
    }

    // 4. Запуск фоновых сервисов
    albus::network::iroh_service::spawn_iroh_receiver(
        &rt,
        iroh_p2p.clone(),
        room.clone(),
        playback_tx.clone(),
        is_host.clone(),
        friends.clone(),
    );
    albus::network::broadcast::spawn_audio_broadcast(&rt, packet_rx, iroh_p2p.clone(), my_id);

    let migration_loop = migration.clone();
    let is_host_loop = is_host.clone();
    rt.spawn(async move {
        println!("[МИГРАЦИЯ] Фоновый цикл Heartbeat/Failover запущен.");
        migration_loop.start_heartbeat_loop(is_host_loop).await;
    });

    // 5. Настройка окна, размера и иконки приложения
    let icon_data = eframe::icon_data::from_png_bytes(include_bytes!("../assets/icon.png")).ok();

    let mut native_options = eframe::NativeOptions::default();
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([480.0, 660.0])
        .with_min_inner_size([430.0, 560.0]);

    if let Some(icon) = icon_data {
        viewport = viewport.with_icon(icon);
    }
    native_options.viewport = viewport;

    // Видеотранспорт: отдельное ALPN, живёт рядом с голосом
    #[cfg(windows)]
    let video_link = iroh_p2p
        .as_ref()
        .map(|p2p| albus::network::video_link::VideoLink::spawn(p2p.clone()));
    #[cfg(windows)]
    let rt_handle = rt.handle().clone();
    #[cfg(windows)]
    let video_link_gui = video_link.clone();

    let _guard = rt.enter();
    let _runtime_guard = rt;
    eframe::run_native(
        "Albus Audio",
        native_options,
        Box::new(move |_cc| {
            let mut app = RadioApp::new(
                mixer,
                room,
                config.ptt_vk,
                ptt_flag,
                selected_input,
                selected_output,
                iroh_p2p,
                my_id,
                mic_stream,
                packet_tx,
                input_devices,
                input_channel_counts,
                output_devices,
                is_host,
                playback_stream,
                playback_rx,
                config,
                friends,
                my_node_id,
                network_error,
            );

            #[cfg(windows)]
            if let Some(link) = video_link_gui {
                app.attach_video(link, rt_handle.clone());
            }

            Ok(Box::new(app))
        }),
    )
    .unwrap();
}
