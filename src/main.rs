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
    // Общий чат комнаты: история тоже только в памяти
    let room_chat = Arc::new(albus::chat::RoomChat::new());
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
    // Каталог трансляций: издатель кладёт туда номер своей, зрители — анонсы
    // соседей. Общий для миграции, сервиса приёма и видео-транспорта.
    let video_directory = Arc::new(albus::network::video_directory::VideoDirectory::new());
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
    if let Some(bps) = config.opus_bitrate {
        mixer.set_opus_bitrate(bps);
    }
    for (id, vol) in &config.peer_volumes {
        mixer.set_peer_volume(*id, *vol);
    }
    // Эффекты — опция, по умолчанию выключены. Старый конфиг знал только
    // общий тумблер «обработка звука» и профиль: из них выводим начальное
    // состояние независимых тумблеров, новые ключи важнее.
    let legacy_dsp = config.mic_processing.unwrap_or(false);
    let legacy_effects = |profile: Option<u32>, default: albus::mixer::SlotProfile| {
        let profile = profile
            .map(albus::mixer::SlotProfile::from_u32)
            .unwrap_or(default);
        legacy_dsp && profile.uses_voice_dsp()
    };
    let voice1 = legacy_effects(
        config.input1_profile,
        albus::mixer::SlotProfile::Voice,
    );
    let voice2 = legacy_effects(
        config.input2_profile,
        albus::mixer::SlotProfile::Instrument,
    );
    mixer.set_input1_denoise(config.input1_denoise.unwrap_or(voice1));
    mixer.set_input1_gate(config.input1_gate.unwrap_or(voice1));
    mixer.set_input1_agc(config.input1_agc.unwrap_or(voice1));
    mixer.set_input2_denoise(config.input2_denoise.unwrap_or(voice2));
    mixer.set_input2_gate(config.input2_gate.unwrap_or(voice2));
    mixer.set_input2_agc(config.input2_agc.unwrap_or(voice2));
    // Эхо-подавление — не часть профилей и старый конфиг о нём не знал:
    // по умолчанию выключено.
    mixer.set_input1_aec(config.input1_aec.unwrap_or(false));
    mixer.set_input2_aec(config.input2_aec.unwrap_or(false));
    mixer.set_tx_limiter_enabled(config.tx_limiter.unwrap_or(legacy_dsp));
    mixer.set_rx_limiter_enabled(config.rx_limiter.unwrap_or(false));
    if let Some(vk) = config.ptt_vk {
        albus::keybind::CURRENT_VK.store(vk, std::sync::atomic::Ordering::Relaxed);
    }
    let mut audio = UniversalAudioCapture::new().unwrap();
    let migration = Arc::new(MigrationManager::new(
        my_id,
        room.clone(),
        iroh_p2p.clone(),
        video_directory.clone(),
    ));

    // 3. Каналы передачи аудио
    let (packet_tx, packet_rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);
    let (playback_tx, playback_rx) = tokio::sync::mpsc::channel::<(u64, u32, Vec<u8>)>(100);
    let playback_rx = Arc::new(std::sync::Mutex::new(playback_rx));
    let playback_stream = Arc::new(Mutex::new(None));
    // Общая точка диагностики: счётчики приёма пишет аудиоколбэк, показывает
    // панель «Диагностика» и копирует в отчёт. Живёт всё время работы.
    let diagnostics = Arc::new(albus::diagnostics::Diagnostics::new());

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
        diagnostics.clone(),
    ) {
        Ok(stream) => *playback_stream.lock().unwrap() = Some(stream),
        Err(e) => eprintln!(
            "[ОШИБКА] Динамики не открылись: {e}. Собеседников не будет слышно — выберите другое устройство вывода в настройках."
        ),
    }

    // Один Opus-энкодер и один поток нарезки кадров — общие для обоих входов.
    let tx = albus::txbus::TxContext::new(mixer.clone(), ptt_flag.clone(), packet_tx.clone());

    // Входы: либо один поток с двух каналов одной карты (как раньше), либо два
    // потока с разных устройств. Второй открываем только когда он выбран и
    // действительно отличается от первого.
    let configured_input = config
        .input_device
        .as_deref()
        .and_then(DeviceSelector::find_input_device_by_name);
    let second_name = config
        .input2_device
        .as_deref()
        .filter(|name| config.input_device.as_deref() != Some(*name));
    let mic_stream2 = Arc::new(Mutex::new(None));
    let mut separate_inputs = false;
    if let Some(second) = second_name {
        match DeviceSelector::find_input_device_by_name(second) {
            Some(second_device) => {
                separate_inputs = true;
                let epochs1 = tx.bus.next_epochs([true, false]);
                match audio.create_input_stream(
                    configured_input.clone(),
                    tx.clone(),
                    [true, false],
                    epochs1,
                ) {
                    Ok(stream) => {
                        tx.bus.commit_epochs([true, false], epochs1);
                        *mic_stream.lock().unwrap() = Some(stream);
                    }
                    Err(e) => eprintln!(
                        "[ОШИБКА] Вход 1 не открылся: {e}. Вас не будет слышно — выберите другое устройство ввода в настройках."
                    ),
                }
                let epochs2 = tx.bus.next_epochs([false, true]);
                match audio.create_input_stream(
                    Some(second_device),
                    tx.clone(),
                    [false, true],
                    epochs2,
                ) {
                    Ok(stream) => {
                        tx.bus.commit_epochs([false, true], epochs2);
                        *mic_stream2.lock().unwrap() = Some(stream);
                    }
                    Err(e) => eprintln!(
                        "[ОШИБКА] Вход 2 ('{}') не открылся: {e}. Он останется молчать, пока устройство не освободится.",
                        second
                    ),
                }
            }
            None => eprintln!(
                "[АУДИО] Второй вход '{}' не найден — оба канала берутся с первой карты.",
                second
            ),
        }
    }
    if !separate_inputs {
        let epochs = tx.bus.next_epochs([true, true]);
        match audio.create_input_stream(configured_input, tx.clone(), [true, true], epochs) {
            Ok(stream) => {
                tx.bus.commit_epochs([true, true], epochs);
                *mic_stream.lock().unwrap() = Some(stream);
            }
            Err(e) => eprintln!(
                "[ОШИБКА] Микрофон не открылся: {e}. Вас не будет слышно — выберите другое устройство ввода в настройках."
            ),
        }
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

    // Онлайн-статус друзей: кто недавно подавал признаки жизни
    let presence = Arc::new(albus::presence::PresenceTracker::new());

    // 4. Запуск фоновых сервисов
    albus::network::iroh_service::spawn_iroh_receiver(
        &rt,
        iroh_p2p.clone(),
        room.clone(),
        playback_tx.clone(),
        is_host.clone(),
        friends.clone(),
        chat_log.clone(),
        room_chat.clone(),
        calls.clone(),
        presence.clone(),
        video_directory.clone(),
    );
    if let Some(ref p2p) = iroh_p2p {
        albus::presence::spawn_ping_loop(
            rt.handle(),
            p2p.clone(),
            friends.clone(),
            presence.clone(),
            my_id,
        );
    }
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
    // Хост комнаты принимает feed издателя и раздаёт его зрителям: транспорт
    // должен знать, хост ли мы, и не обслуживать запросы после «Выйти».
    let video_link = iroh_p2p.as_ref().map(|p2p| {
        albus::network::video_link::VideoLink::spawn(
            p2p.clone(),
            rt_handle.clone(),
            video_directory.clone(),
            is_host.clone(),
            room.clone(),
        )
    });
    #[cfg(windows)]
    let video_link_gui = video_link.clone();
    #[cfg(windows)]
    let video_directory_gui = video_directory.clone();

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
                mic_stream2,
                tx,
                input_devices,
                input_channel_counts,
                output_devices,
                is_host,
                playback_stream,
                playback_rx,
                config,
                friends,
                chat_log,
                room_chat,
                my_node_id,
                network_error,
                calls,
                presence,
                diagnostics,
            );

            #[cfg(windows)]
            if let Some(link) = video_link_gui {
                app.attach_video(link, video_directory_gui.clone());
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
