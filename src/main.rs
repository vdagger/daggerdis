use albus::{
    audio::UniversalAudioCapture, devices::DeviceSelector, gui::RadioApp,
    migration::MigrationManager, mixer::AudioMixer, network::p2p_iroh::IrohP2pManager,
    room::RoomManager,
};
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, Mutex};

fn main() {
    // Паника в GUI-приложении иначе выглядит как «окно не открылось»:
    // пишем её в %TEMP%\albus-panic.log, чтобы было что показать.
    std::panic::set_hook(Box::new(|info| {
        let text = format!("[ПАНИКА] {info}\n");
        eprintln!("{text}");
        let path = std::env::temp_dir().join("albus-panic.log");
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            use std::io::Write;
            let _ = file.write_all(text.as_bytes());
        }
    }));

    // 1. Автоматическая настройка правил фаервола Windows (входящий UDP для iroh QUIC)
    albus::firewall::setup_firewall_rules();

    // Штамп сборки первым делом: у двух собеседников он должен совпадать,
    // иначе один из них запускает старый exe и «робот» ищется не там.
    println!(
        "[СБОРКА] Albus {} · сборка {}",
        env!("CARGO_PKG_VERSION"),
        env!("ALBUS_BUILD_STAMP")
    );

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
    let chat_log = Arc::new(albus::chat::ChatLog::new());
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
    // Обработка звука — опция, по умолчанию выключена
    mixer.set_dsp_enabled(config.mic_processing.unwrap_or(false));
    mixer.set_rx_limiter_enabled(config.rx_limiter.unwrap_or(false));
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
    // Ошибку вывода раньше проглатывал `if let Ok`: при несовпадении формата
    // динамиков cpal возвращает StreamConfigNotSupported, поток не открывался,
    // и собеседник просто ничего не слышал — в консоли при этом было пусто.
    match albus::audio::UniversalAudioCapture::create_playback_stream(
        configured_output,
        playback_rx.clone(),
        mixer.clone(),
    ) {
        Ok(stream) => *playback_stream.lock().unwrap() = Some(stream),
        Err(e) => eprintln!(
            "[ОШИБКА] Динамики не открылись: {e}. Собеседников не будет слышно — выберите другое устройство вывода в настройках."
        ),
    }

    let configured_input = config
        .input_device
        .as_deref()
        .and_then(DeviceSelector::find_input_device_by_name);
    match audio.create_input_stream(
        configured_input,
        ptt_flag.clone(),
        packet_tx.clone(),
        mixer.clone(),
    ) {
        Ok(stream) => *mic_stream.lock().unwrap() = Some(stream),
        Err(e) => eprintln!(
            "[ОШИБКА] Микрофон не открылся: {e}. Вас не будет слышно — выберите другое устройство ввода в настройках."
        ),
    }

    // Звонки: приглашение → «Принять» → разговор; до согласия собеседник
    // не попадает в комнату
    let calls = Arc::new(albus::calls::CallManager::new(
        my_id,
        config.nick.clone().unwrap_or_else(|| "Гость".to_string()),
        iroh_p2p.clone(),
        room.clone(),
        is_host.clone(),
    ));

    // 4. Запуск фоновых сервисов
    albus::network::iroh_service::spawn_iroh_receiver(
        &rt,
        iroh_p2p.clone(),
        room.clone(),
        playback_tx.clone(),
        is_host.clone(),
        friends.clone(),
        chat_log.clone(),
        calls.clone(),
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
    // По умолчанию — glow (OpenGL), как и раньше: на обычной машине он легче.
    // В виртуалках и RDP OpenGL 2.0 может отсутствовать; там есть wgpu,
    // включаемый переменной ALBUS_RENDERER=wgpu. Без явного указания eframe
    // при включённой feature wgpu выбрал бы wgpu сам.
    native_options.renderer = eframe::Renderer::Glow;
    if std::env::var("ALBUS_RENDERER")
        .map(|v| v.eq_ignore_ascii_case("wgpu"))
        .unwrap_or(false)
    {
        native_options.renderer = eframe::Renderer::Wgpu;
    }
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1120.0, 700.0])
        .with_min_inner_size([980.0, 620.0]);

    if let Some(icon) = icon_data {
        viewport = viewport.with_icon(icon);
    }
    native_options.viewport = viewport;

    // Видеотранспорт: отдельное ALPN, живёт рядом с голосом
    #[cfg(windows)]
    let rt_handle = rt.handle().clone();
    #[cfg(windows)]
    let video_link = iroh_p2p.as_ref().map(|p2p| {
        albus::network::video_link::VideoLink::spawn(p2p.clone(), rt_handle.clone())
    });
    #[cfg(windows)]
    let video_link_gui = video_link.clone();

    let _guard = rt.enter();
    let _runtime_guard = rt;
    let result = eframe::run_native(
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
                chat_log,
                my_node_id,
                network_error,
                calls,
            );

            #[cfg(windows)]
            if let Some(link) = video_link_gui {
                app.attach_video(link, rt_handle.clone());
            }

            Ok(Box::new(app))
        }),
    );
    // Без этого любая ошибка окна (нет OpenGL, нет GPU) выглядела как
    // «программа не запускается»: процесс умирал молча.
    if let Err(e) = result {
        let msg = format!(
            "Окно не открылось: {e}\n\nЕсли это виртуалка или RDP без OpenGL 2.0, \
             запустите с переменной окружения ALBUS_RENDERER=wgpu."
        );
        eprintln!("{msg}");
        show_fatal(&msg);
        std::process::exit(1);
    }
}

/// Ошибка старта видна пользователю, даже когда консоли нет.
#[cfg(windows)]
fn show_fatal(message: &str) {
    const MB_OK: u32 = 0x0;
    const MB_ICONERROR: u32 = 0x10;
    extern "system" {
        fn MessageBoxW(hwnd: *mut core::ffi::c_void, text: *const u16, caption: *const u16, utype: u32) -> i32;
    }
    let to_utf16 = |s: &str| -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    };
    let text = to_utf16(message);
    let caption = to_utf16("Albus Audio");
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            text.as_ptr(),
            caption.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn show_fatal(message: &str) {
    eprintln!("{message}");
}
