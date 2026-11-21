//! Графический интерфейс: компактная эфирная консоль с оверлеями

pub mod theme;

mod stream_control;

#[cfg(windows)]
mod video_session;

use eframe::egui;
use egui::{Align, FontFamily, FontId, Layout, Order, Rounding, Stroke, TextStyle};
use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use theme as t;

pub struct RadioApp {
    mixer: Arc<crate::mixer::AudioMixer>,
    ptt_bind: Option<crate::keybind::PttBind>,
    ptt_flag: Arc<AtomicBool>,
    room: Arc<crate::room::RoomManager>,
    connect_ip_input: String,
    selected_input: Arc<std::sync::atomic::AtomicU32>,
    selected_output: Arc<std::sync::atomic::AtomicU32>,
    iroh: Option<Arc<crate::network::p2p_iroh::IrohP2pManager>>,
    my_id: u64,
    mic_stream: Arc<Mutex<Option<cpal::Stream>>>,
    /// Второй физический вход отдельным потоком; `None` — слот 2 читает
    /// первый поток (две дорожки одной карты).
    mic_stream2: Arc<Mutex<Option<cpal::Stream>>>,
    /// Шина кадров: общий Opus-энкодер и нарезка по 20 мс для обоих входов.
    tx: crate::txbus::TxContext,
    /// Устройство, с которого сейчас собран вход 1. Сравнение идёт по имени:
    /// индексы в списках сдвигаются при подключении/выдёргивании карт, и по
    /// индексу поток перезапускался бы на том же самом устройстве.
    prev_input_name: Option<String>,
    /// Выбор второго входа: 0 — «то же устройство», n — устройство n-1.
    selected_input2: Arc<std::sync::atomic::AtomicU32>,
    prev_input2_name: Option<String>,
    /// Почему второй вход молчит (занят, выдернут) — строкой в настройках.
    input2_error: Option<String>,
    input_devices: Vec<String>,
    input_channel_counts: Vec<u32>,
    output_devices: Vec<String>,
    /// Фоновый обход подключения/отключения карт (см. `devices::DeviceWatcher`).
    device_watch: crate::devices::DeviceWatcher,
    is_host: Arc<AtomicBool>,
    playback_stream: Arc<Mutex<Option<cpal::Stream>>>,
    playback_rx: Arc<std::sync::Mutex<tokio::sync::mpsc::Receiver<(u64, u32, Vec<u8>)>>>,
    /// Динамики, на которых сейчас собран поток воспроизведения (сравнение по
    /// имени — см. `prev_input_name`).
    prev_output_name: Option<String>,
    continuous_mode: bool,
    theme_configured: bool,
    settings_open: bool,
    chat_open: bool,
    /// В чате открыта вкладка «Комната», а не переписка с другом.
    chat_room: bool,
    /// Панель «Диагностика»: числа и «Скопировать отчёт».
    diag_open: bool,
    /// Общая точка диагностики: счётчики приёма голоса пишет аудиоколбэк.
    diagnostics: Arc<crate::diagnostics::Diagnostics>,
    /// Прошлый снимок счётчиков приёма и время: по ним считается «за N с».
    diag_prev: (crate::diagnostics::RxSnapshot, Instant),
    /// Прирост счётчиков за последнее окно — показывается в панели.
    diag_window: crate::diagnostics::RxSnapshot,
    diag_window_secs: f32,
    /// Сколько кадров чужой трансляции принято (для отчёта).
    remote_frames: u64,
    toast: Option<(String, Instant)>,
    wave: VecDeque<f32>,
    config: crate::config::AppConfig,
    friends: Arc<crate::friends::FriendsManager>,
    chat_log: Arc<crate::chat::ChatLog>,
    /// Общий чат комнаты: сообщение видят все в эфире, а не один друг.
    room_chat: Arc<crate::chat::RoomChat>,
    calls: Arc<crate::calls::CallManager>,
    /// Кто из друзей недавно подавал признаки жизни (ping/pong).
    presence: Arc<crate::presence::PresenceTracker>,
    active_chat: Option<String>,
    /// Собеседник, для которого сейчас набран `chat_input`. Смена диалога
    /// чистит черновик, а не переносит его в чужую переписку.
    chat_target: Option<String>,
    chat_input: String,
    /// Ошибка отправки в чат из фоновой задачи: `update` забирает её тостом.
    /// Сообщение уже нарисовано в ленте, поэтому молчать об ошибке нельзя.
    chat_error: Arc<std::sync::Mutex<Option<String>>>,
    /// Счётчики непрочитанного на прошлом кадре: рост = новое сообщение,
    /// о нём сообщаем тостом, потому что бейдж легко пропустить.
    seen_unread: std::collections::HashMap<String, usize>,
    /// Окно уже мигнуло в панели задач за текущий входящий звонок.
    call_attention: bool,
    /// Выход через меню трея: закрытие окна больше не перехватываем.
    tray_quit: bool,
    /// Окно спрятано в трей — нужно, чтобы опрашивать команды иконки.
    hidden_to_tray: bool,
    /// Автозапуск при входе в Windows (кэш: reg.exe не дёргаем каждый кадр).
    autostart_on: bool,
    my_node_id: String,
    network_error: Option<String>,
    nick: String,
    add_friend_node: String,
    reset_note: Option<String>,
    pending_save: bool,
    last_save: Instant,
    #[cfg(windows)]
    title_dark: bool,
    #[cfg(windows)]
    video: Option<video_session::VideoSession>,
    #[cfg(windows)]
    video_error: Option<String>,
    #[cfg(windows)]
    video_fullscreen: bool,
    #[cfg(windows)]
    video_window: bool,
    #[cfg(windows)]
    remote: Option<video_session::RemoteView>,
    /// Счётчик сбросов очереди зрителя на прошлом кадре: сменился — прошлую
    /// картинку не показываем, даже если источник тот же.
    #[cfg(windows)]
    remote_epoch_seen: u64,
    #[cfg(windows)]
    video_link: Option<Arc<crate::network::video_link::VideoLink>>,
    /// Кто в комнате сейчас в эфире: по этой карте зритель выбирает поток
    /// и переподписывается при перезапуске чужой трансляции.
    #[cfg(windows)]
    video_directory: Option<Arc<crate::network::video_directory::VideoDirectory>>,
}

impl RadioApp {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mixer: Arc<crate::mixer::AudioMixer>,
        room: Arc<crate::room::RoomManager>,
        ptt_bind: Option<crate::keybind::PttBind>,
        ptt_flag: Arc<AtomicBool>,
        selected_input: Arc<std::sync::atomic::AtomicU32>,
        selected_output: Arc<std::sync::atomic::AtomicU32>,
        iroh: Option<Arc<crate::network::p2p_iroh::IrohP2pManager>>,
        my_id: u64,
        mic_stream: Arc<Mutex<Option<cpal::Stream>>>,
        mic_stream2: Arc<Mutex<Option<cpal::Stream>>>,
        tx: crate::txbus::TxContext,
        input_devices: Vec<String>,
        input_channel_counts: Vec<u32>,
        output_devices: Vec<String>,
        is_host: Arc<AtomicBool>,
        playback_stream: Arc<Mutex<Option<cpal::Stream>>>,
        playback_rx: Arc<std::sync::Mutex<tokio::sync::mpsc::Receiver<(u64, u32, Vec<u8>)>>>,
        config: crate::config::AppConfig,
        friends: Arc<crate::friends::FriendsManager>,
        chat_log: Arc<crate::chat::ChatLog>,
        room_chat: Arc<crate::chat::RoomChat>,
        my_node_id: String,
        network_error: Option<String>,
        calls: Arc<crate::calls::CallManager>,
        presence: Arc<crate::presence::PresenceTracker>,
        diagnostics: Arc<crate::diagnostics::Diagnostics>,
    ) -> Self {
        let initial_input = selected_input.load(std::sync::atomic::Ordering::Relaxed);
        let initial_output = selected_output.load(std::sync::atomic::Ordering::Relaxed);
        let initial_input2 = config
            .input2_device
            .as_deref()
            .and_then(|name| input_devices.iter().position(|d| d == name))
            .map(|i| i as u32 + 1)
            .unwrap_or(0);
        let nick = config.nick.clone().unwrap_or_else(|| "Гость".to_string());

        // Устройства, с которыми приложение стартовало: дальше вход и выход
        // сравниваются с ними по имени, а не по индексу.
        let initial_input_name = input_devices.get(initial_input as usize).cloned();
        let initial_input2_name = if initial_input2 == 0 {
            None
        } else {
            input_devices.get(initial_input2 as usize - 1).cloned()
        };
        let initial_output_name = output_devices.get(initial_output as usize).cloned();

        Self {
            mixer,
            ptt_bind,
            ptt_flag,
            room,
            connect_ip_input: String::new(),
            selected_input,
            selected_output,
            iroh,
            my_id,
            mic_stream,
            mic_stream2,
            tx,
            prev_input_name: initial_input_name,
            selected_input2: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(initial_input2)),
            prev_input2_name: initial_input2_name,
            input2_error: None,
            input_devices,
            input_channel_counts,
            output_devices,
            device_watch: crate::devices::DeviceWatcher::spawn(),
            is_host,
            playback_stream,
            playback_rx,
            prev_output_name: initial_output_name,
            continuous_mode: config.continuous_mode.unwrap_or(false),
            theme_configured: false,
            settings_open: false,
            chat_open: false,
            chat_room: false,
            diag_open: false,
            diagnostics: diagnostics.clone(),
            diag_prev: (diagnostics.rx.snapshot(), Instant::now()),
            diag_window: crate::diagnostics::RxSnapshot::default(),
            diag_window_secs: 0.0,
            remote_frames: 0,
            toast: None,
            wave: VecDeque::new(),
            config,
            friends,
            chat_log,
            room_chat,
            calls,
            presence,
            active_chat: None,
            chat_target: None,
            chat_input: String::new(),
            chat_error: Arc::new(std::sync::Mutex::new(None)),
            seen_unread: std::collections::HashMap::new(),
            call_attention: false,
            tray_quit: false,
            hidden_to_tray: false,
            autostart_on: crate::autostart::is_enabled(),
            my_node_id,
            network_error,
            nick,
            add_friend_node: String::new(),
            reset_note: None,
            pending_save: false,
            last_save: Instant::now(),
            #[cfg(windows)]
            title_dark: false,
            #[cfg(windows)]
            video: None,
            #[cfg(windows)]
            video_error: None,
            #[cfg(windows)]
            video_fullscreen: false,
            #[cfg(windows)]
            video_window: false,
            #[cfg(windows)]
            remote: None,
            #[cfg(windows)]
            remote_epoch_seen: 0,
            #[cfg(windows)]
            video_link: None,
            #[cfg(windows)]
            video_directory: None,
        }
    }

    /// Подключает видео к сети: транспорт и рантайм для исходящих соединений.
    #[cfg(windows)]
    pub fn attach_video(
        &mut self,
        link: Arc<crate::network::video_link::VideoLink>,
        directory: Arc<crate::network::video_directory::VideoDirectory>,
    ) {
        self.video_link = Some(link);
        self.video_directory = Some(directory);
    }

    /// Подменяет входные потоки, когда в настройках выбрали другие устройства.
    /// Второй поток существует только при отдельном устройстве; «То же
    /// устройство» снова собирает один поток с двух каналов первой карты.
    ///
    /// Сравнение идёт по именам устройств. Список обновляется на лету
    /// (`refresh_devices`), и после подключения карты прежний индекс начинает
    /// указывать на другое устройство: по индексу поток перезапускался бы на
    /// том же самом микрофоне и зря рвал бы звук.
    fn reconcile_inputs(&mut self) {
        let current_input = self
            .selected_input
            .load(std::sync::atomic::Ordering::Relaxed);
        let current_input2 = self
            .selected_input2
            .load(std::sync::atomic::Ordering::Relaxed);

        let Some(device1) = self.input_devices.get(current_input as usize).cloned() else {
            return;
        };
        let device2 = if current_input2 == 0 {
            None
        } else {
            self.input_devices
                .get(current_input2 as usize - 1)
                .cloned()
        };
        if self.prev_input_name.as_deref() == Some(device1.as_str())
            && self.prev_input2_name == device2
        {
            return;
        }
        self.prev_input_name = Some(device1.clone());
        self.prev_input2_name = device2.clone();

        let separate = device2
            .as_deref()
            .map_or(false, |name| Some(name) != Some(device1.as_str()));

        self.input2_error = None;
        if separate {
            let device2 = device2.unwrap_or_default();
            // Первый поток с отдельным вторым устройством заполняет только
            // свой слот: иначе канал первой карты попал бы в обе дорожки.
            if let Err(e) = stream_control::restart_audio_stream(
                &self.mic_stream,
                crate::audio::UniversalAudioCapture::new().unwrap(),
                self.tx.clone(),
                [true, false],
                Some(&device1),
                true,
            ) {
                self.show_toast(&format!("Вход 1 не открылся: {}", e));
            }
            // Второму входу микрофон по умолчанию не подставляем: в инструмент
            // молча пошёл бы не тот источник.
            if let Err(e) = stream_control::restart_audio_stream(
                &self.mic_stream2,
                crate::audio::UniversalAudioCapture::new().unwrap(),
                self.tx.clone(),
                [false, true],
                Some(&device2),
                false,
            ) {
                self.input2_error = Some(e);
            }
            self.config.input_device = Some(device1);
            self.config.input2_device = Some(device2);
        } else {
            // Сначала гасим второй поток, иначе он продолжит дописывать в слот 2
            // поверх первого и смесь станет вдвое громче.
            match self.mic_stream2.lock() {
                Ok(mut s) => *s = None,
                Err(p) => *p.into_inner() = None,
            }
            if let Err(e) = stream_control::restart_audio_stream(
                &self.mic_stream,
                crate::audio::UniversalAudioCapture::new().unwrap(),
                self.tx.clone(),
                [true, true],
                Some(&device1),
                true,
            ) {
                self.show_toast(&format!("Микрофон не открылся: {}", e));
            }
            self.config.input_device = Some(device1);
            self.config.input2_device = None;
        }
        self.pending_save = true;
    }

    /// Забирает свежий список устройств, если фоновый обход увидел изменения.
    ///
    /// Горячее подключение (hot-plug): карту могут выдернуть или вставить при
    /// работающем приложении. Раньше список снимался один раз при старте, и
    /// новый USB-микрофон появлялся только после перезапуска, а выдернутая
    /// карта оставляла выбор на устройстве, которого уже нет.
    ///
    /// Выбор хранится по имени (в `config.ini` тоже), поэтому после обновления
    /// индексы пересчитываются по именам: «USB-микрофон» не превращается молча
    /// в «Стереомикшер», когда список сдвинулся. Если выбранного устройства
    /// больше нет — откатываемся на актуальное умолчание и говорим тостом,
    /// иначе пропажа звука выглядит беспричинной.
    fn refresh_devices(&mut self) {
        let Some(snapshot) = self.device_watch.try_recv() else {
            return;
        };
        let relaxed = std::sync::atomic::Ordering::Relaxed;
        let selected_in = self.selected_input.load(relaxed) as usize;
        let selected_in2 = self.selected_input2.load(relaxed) as usize;
        let selected_out = self.selected_output.load(relaxed) as usize;

        // План считает `devices::plan_device_update` — там же его правила и
        // тесты: пустой снимок не принимаем, выбор пересчитываем по имени,
        // пропавшее устройство откатываем.
        let Some(plan) = crate::devices::plan_device_update(
            &self.input_devices,
            &self.output_devices,
            selected_in,
            selected_in2,
            selected_out,
            snapshot,
        ) else {
            return;
        };

        // Журнал не только про выбранное устройство: «карта пропала, а звук
        // работает» — первый вопрос при разборе, и без этих строк непонятно,
        // видит ли приложение изменения вообще.
        if !plan.removed.is_empty() {
            eprintln!("[АУДИО] Устройства отключены: {}", plan.removed.join(", "));
        }
        if !plan.added.is_empty() {
            eprintln!("[АУДИО] Устройства подключены: {}", plan.added.join(", "));
        }
        if !plan.gone.is_empty() {
            eprintln!("[АУДИО] Выбранного устройства нет: {}", plan.gone.join(", "));
        }

        self.input_devices = plan.next.inputs;
        self.input_channel_counts = plan.next.input_channels;
        self.output_devices = plan.next.outputs;

        match plan.input {
            crate::devices::Reselection::Same(index) => {
                self.selected_input.store(index as u32, relaxed)
            }
            crate::devices::Reselection::Gone(fallback) => {
                self.selected_input.store(fallback.unwrap_or(0) as u32, relaxed)
            }
        }
        match plan.input2 {
            crate::devices::Reselection::Same(index) => {
                self.selected_input2.store(index as u32 + 1, relaxed)
            }
            crate::devices::Reselection::Gone(_) => self.selected_input2.store(0, relaxed),
        }
        match plan.output {
            crate::devices::Reselection::Same(index) => {
                self.selected_output.store(index as u32, relaxed)
            }
            crate::devices::Reselection::Gone(fallback) => {
                self.selected_output.store(fallback.unwrap_or(0) as u32, relaxed)
            }
        }

        if !plan.gone.is_empty() {
            self.show_toast(&format!("Устройство отключено: {}", plan.gone.join(", ")));
        } else if !plan.added.is_empty() {
            self.show_toast(&format!("Подключено: {}", plan.added.join(", ")));
        }
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        if self.theme_configured {
            return;
        }

        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = t::BG;
        visuals.window_fill = t::SURFACE;
        visuals.extreme_bg_color = t::INSET;
        visuals.faint_bg_color = t::SURFACE_HI;
        visuals.window_stroke = Stroke::new(1.0_f32, t::STROKE_SOFT);
        visuals.window_rounding = Rounding::same(16.0);
        visuals.menu_rounding = Rounding::same(10.0);
        visuals.window_shadow = egui::epaint::Shadow {
            offset: egui::vec2(0.0, 16.0),
            blur: 44.0,
            spread: 0.0,
            color: egui::Color32::from_black_alpha(170),
        };
        visuals.popup_shadow = visuals.window_shadow;

        visuals.widgets.inactive.bg_fill = t::with_alpha(egui::Color32::WHITE, 0.04);
        visuals.widgets.inactive.weak_bg_fill = t::SURFACE;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, t::STROKE_SOFT);
        visuals.widgets.inactive.rounding = Rounding::same(8.0);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, t::TEXT_DIM);

        visuals.widgets.hovered.bg_fill = t::with_alpha(egui::Color32::WHITE, 0.07);
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, t::with_alpha(t::BRAND, 0.40));
        visuals.widgets.hovered.rounding = Rounding::same(8.0);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, t::TEXT);

        visuals.widgets.active.bg_fill = t::with_alpha(t::BRAND, 0.20);
        visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, t::BRAND);
        visuals.widgets.active.rounding = Rounding::same(8.0);
        visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, t::TEXT);

        visuals.selection.bg_fill = t::with_alpha(t::BRAND, 0.32);
        visuals.selection.stroke = Stroke::new(1.0_f32, t::TEXT);
        visuals.override_text_color = Some(t::TEXT);
        visuals.hyperlink_color = t::BRAND;
        visuals.slider_trailing_fill = true;

        ctx.set_visuals(visuals);

        let mut style = (*ctx.style()).clone();
        style.text_styles = [
            (TextStyle::Heading, FontId::new(18.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(12.5, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(11.5, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(10.0, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(10.5, FontFamily::Monospace)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(7.0, 6.0);
        style.spacing.button_padding = egui::vec2(8.0, 4.0);
        style.spacing.slider_width = 140.0;
        style.spacing.window_margin = egui::Margin::same(14.0);
        style.spacing.interact_size.y = 22.0;
        ctx.set_style(style);

        self.theme_configured = true;
    }

    /// Тёмный системный заголовок окна — иначе белая полоса ломает тёмный макет
    #[cfg(windows)]
    fn apply_dark_titlebar(&mut self, frame: &eframe::Frame) {
        if self.title_dark {
            return;
        }
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let Ok(handle) = frame.window_handle() else {
            return;
        };
        let RawWindowHandle::Win32(w) = handle.as_raw() else {
            return;
        };
        let hwnd = windows::Win32::Foundation::HWND(w.hwnd.get() as *mut std::ffi::c_void);
        let on: i32 = 1;
        let res = unsafe {
            windows::Win32::Graphics::Dwm::DwmSetWindowAttribute(
                hwnd,
                windows::Win32::Graphics::Dwm::DWMWA_USE_IMMERSIVE_DARK_MODE,
                &on as *const i32 as *const std::ffi::c_void,
                std::mem::size_of::<i32>() as u32,
            )
        };
        self.title_dark = res.is_ok();
        // Трею нужен HWND, чтобы показывать окно, пока сам GUI спит.
        crate::tray::set_main_window(w.hwnd.get() as isize);
    }

    fn show_toast(&mut self, text: &str) {
        self.toast = Some((text.to_string(), Instant::now()));
    }

    fn copy_node_id(&mut self, ctx: &egui::Context) {
        ctx.output_mut(|o| o.copied_text = self.my_node_id.clone());
        self.show_toast("NodeID скопирован — отправьте другу");
    }

    /// Раз в секунду пересчитывает окно счётчиков приёма.
    ///
    /// Накопительные числа отвечают на вопрос «что было за сессию», а окно —
    /// «что происходит прямо сейчас»: при разборе «робота» важно второе.
    fn sample_diagnostics(&mut self) {
        let elapsed = self.diag_prev.1.elapsed();
        if elapsed < Duration::from_secs(1) {
            return;
        }
        let current = self.diagnostics.rx.snapshot();
        self.diag_window = self.diag_prev.0.delta(&current);
        self.diag_window_secs = elapsed.as_secs_f32();
        self.diag_prev = (current, Instant::now());
        self.tune_voice();
    }

    /// Подстраивает передачу голоса под сеть раз в секунду.
    ///
    /// Два числа, которые видит только транспорт: доля потерянных пакетов на
    /// наших путях и оценка полосы. По ним Opus получает ожидаемые потери
    /// (чем больше, тем плотнее защита — in-band FEC и т.п.), а если полосы
    /// явно не хватает на всех собеседников, потолок битрейта опускается на
    /// ступень. Настройку пользователя не трогаем: она остаётся верхней
    /// границей, адаптация живёт отдельным потолком и снимается, когда сеть
    /// наладилась.
    fn tune_voice(&mut self) {
        let Some(p2p) = self.iroh.clone() else {
            return;
        };
        let links = p2p.link_stats();
        if links.is_empty() {
            return;
        }
        let mut sent = 0u64;
        let mut lost = 0u64;
        let mut min_bw = u64::MAX;
        for link in &links {
            sent += link.sent_packets;
            lost += link.lost_packets;
            if let Some(bps) = link.bandwidth_bps {
                min_bw = min_bw.min(bps);
            }
        }

        let pct = crate::diagnostics::loss_percent(sent, lost).min(30) as i64;
        let current = self.mixer.packet_loss_perc() as i64;
        // Сглаживание: один процент туда-обратно — шум измерения, а не сигнал.
        if (pct - current).abs() >= 4 {
            self.mixer.set_packet_loss_perc(pct as u32);
            eprintln!(
                "[АУДИО] Потери на путях {}% → защита Opus {}%",
                pct,
                self.mixer.packet_loss_perc()
            );
        }

        let needed = self.mixer.opus_bitrate() as u64 * links.len().max(1) as u64;
        let cap = if min_bw != u64::MAX && min_bw < needed * 13 / 10 {
            crate::mixer::next_voice_bitrate_below(self.mixer.opus_bitrate())
                .unwrap_or(self.mixer.opus_bitrate())
        } else {
            u32::MAX
        };
        if cap != self.mixer.voice_bitrate_cap() {
            self.mixer.set_voice_bitrate_cap(cap);
            if cap == u32::MAX {
                println!("[АУДИО] Полосы хватает — битрейт передачи вернулся к настройке");
            } else {
                println!(
                    "[АУДИО] Полосы мало на {} путей → битрейт передачи {} кбит/с",
                    links.len(),
                    cap / 1000
                );
            }
        }
    }

    /// Всё, что попадает в отчёт и в панель: собирается в одном месте, чтобы
    /// экран и скопированный текст показывали одни и те же числа.
    fn diagnostics_input(&self) -> crate::diagnostics::ReportInput {
        let relaxed = std::sync::atomic::Ordering::Relaxed;
        let friends = self.friends.friends();
        let friends_online = friends
            .iter()
            .filter(|f| self.presence.is_online(&f.node_id))
            .count();
        let links = self
            .iroh
            .as_ref()
            .map(|p2p| p2p.link_stats())
            .unwrap_or_default();

        let mut errors = Vec::new();
        if let Some(e) = &self.network_error {
            errors.push(e.clone());
        }
        if let Some(e) = self.input2_error.as_deref() {
            errors.push(format!("второй вход: {e}"));
        }

        let mut own_video: Option<crate::diagnostics::OwnVideo> = None;
        let mut remote_video: Option<crate::diagnostics::RemoteVideo> = None;
        #[cfg(windows)]
        {
            if let Some(video) = &self.video {
                let (decode_ms, gpu_ms, image_ms) = video.timings();
                own_video = Some(crate::diagnostics::OwnVideo {
                    show_fps: video.fps(),
                    target_fps: video.info().fps,
                    encode_fps: video.encode_fps(),
                    kbps: video.encode_kbps(),
                    decode_ms,
                    gpu_ms,
                    image_ms,
                });
                if let Some(e) = video.last_error() {
                    errors.push(format!("видео: {e}"));
                }
            }
            if let Some(remote) = &self.remote {
                let (decode_ms, gpu_ms, image_ms) = remote.timings();
                remote_video = Some(crate::diagnostics::RemoteVideo {
                    peer: remote.peer.clone(),
                    fps: remote.fps(),
                    decode_ms,
                    gpu_ms,
                    image_ms,
                    buffer_ms: self.config.video_buffer_ms.unwrap_or(2000),
                    frames: self.remote_frames,
                });
                if let Some(e) = remote.last_error() {
                    errors.push(format!("приём видео: {e}"));
                }
            }
            if let Some(e) = &self.video_error {
                errors.push(format!("видео: {e}"));
            }
        }

        let input2 = self.selected_input2.load(relaxed) as usize;

        crate::diagnostics::ReportInput {
            node_id: self.my_node_id.clone(),
            is_host: self.is_host.load(relaxed),
            // +1 — сам узел: в комнате он тоже участник, и шапка «1 узел»
            // считает так же. Иначе панель и шапка показывали разные числа.
            members_in_room: self.room.get_all_peers().len() + 1,
            left_room: self.room.is_left(),
            friends_online,
            friends_total: friends.len(),
            links,
            rx_total: self.diagnostics.rx.snapshot(),
            rx_window: self.diag_window,
            rx_window_secs: self.diag_window_secs.max(1.0),
            mic_device: self
                .input_devices
                .get(self.selected_input.load(relaxed) as usize)
                .cloned(),
            input2_device: if input2 == 0 {
                None
            } else {
                self.input_devices.get(input2 - 1).cloned()
            },
            output_device: self
                .output_devices
                .get(self.selected_output.load(relaxed) as usize)
                .cloned(),
            mic_muted: self.mixer.is_mic_muted(),
            mic_level: self.mixer.input1_peak().max(self.mixer.input2_peak()),
            opus_kbps: self.mixer.opus_bitrate() / 1000,
            own_video,
            remote_video,
            errors,
            uptime: self.diagnostics.uptime(),
        }
    }

    /// «Скопировать отчёт»: тот же текст, что видно в панели, но целиком и
    /// с полным NodeID — его отправляют собеседнику при разборе «плохо слышно».
    fn copy_diagnostics(&mut self, ctx: &egui::Context) {
        let report = crate::diagnostics::render(&self.diagnostics_input());
        // Заодно в журнал: консоль под рукой у того, кто смотрит процесс.
        println!("{report}");
        ctx.output_mut(|o| o.copied_text = report);
        self.show_toast("Отчёт диагностики скопирован");
    }

    fn unread_total(&self) -> usize {
        let dm: usize = self
            .friends
            .friends()
            .iter()
            .map(|f| self.chat_log.unread(&f.node_id))
            .sum();
        dm + self.room_chat.unread()
    }

    fn open_chat(&mut self) {
        if self.active_chat.is_none() {
            self.active_chat = self.friends.friends().first().map(|f| f.node_id.clone());
        }
        // Друзей может не быть вовсе, а эфир — быть: тогда чат открывается
        // сразу на комнате, иначе кнопка «Чат» выглядела бы сломанной.
        if self.active_chat.is_none() && !self.room.get_all_peers().is_empty() {
            self.chat_room = true;
        }
        self.chat_open = self.active_chat.is_some() || self.chat_room;
        self.settings_open = false;
    }
}

/// Секция рейла со стандартными внутренними отступами
fn rail_section(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::none()
        .inner_margin(egui::Margin::symmetric(14.0, 11.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

/// Волосяная линия секций во всю ширину карточки рейла
fn rail_divider(ui: &mut egui::Ui) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, t::STROKE_SOFT);
}

impl eframe::App for RadioApp {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.apply_theme(ctx);

        // Звонки: повтор приглашений, таймауты, гудок и тосты о результате
        self.calls.set_my_nick(&self.nick);
        self.calls.tick();
        self.mixer.set_ring_mode(self.calls.ring_mode());
        while let Some(note) = self.calls.take_notice() {
            self.show_toast(&note);
        }

        // Ctrl+M — глобальный мьют: флаг тот же, что у тумблера в подвале,
        // поэтому состояние на экране не разъезжается с реальным.
        if crate::keybind::take_mute_toggle() {
            let muted = self.mixer.is_mic_muted();
            self.mixer.set_mic_muted(!muted);
            self.show_toast(if muted {
                "Микрофон включён"
            } else {
                "Микрофон выключен"
            });
        }

        // Смена собеседника не должна притаскивать в поле ввода чужой черновик
        if self.active_chat != self.chat_target {
            self.chat_target = self.active_chat.clone();
            self.chat_input.clear();
        }

        // Непрочитанное: бейдж в шапке легко пропустить, поэтому о новом
        // сообщении в закрытом чате говорим тостом.
        for friend in self.friends.friends() {
            let unread = self.chat_log.unread(&friend.node_id);
            let prev = self.seen_unread.get(&friend.node_id).copied().unwrap_or(0);
            let chat_visible = self.chat_open
                && self.active_chat.as_deref() == Some(friend.node_id.as_str());
            if unread > prev && !chat_visible {
                self.show_toast(&format!("Новое сообщение · {}", friend.name));
            }
            self.seen_unread.insert(friend.node_id.clone(), unread);
        }

        // Ошибка отправки в чат: сообщение уже в ленте, поэтому «тихо» здесь
        // означает «человек уверен, что доставлено».
        let chat_error = {
            let mut slot = match self.chat_error.lock() {
                Ok(s) => s,
                Err(p) => p.into_inner(),
            };
            slot.take()
        };
        if let Some(text) = chat_error {
            self.show_toast(&text);
        }

        // Входящий звонок: подсветить приложение в панели задач, если оно
        // свёрнуто или закрыто другим окном, — иначе звонок пропускают.
        let incoming = match self.calls.view() {
            crate::calls::CallView::Incoming { nick, .. } => Some(nick),
            _ => None,
        };
        if let Some(nick) = incoming {
            if !self.call_attention {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                    egui::UserAttentionType::Critical,
                ));
                self.call_attention = true;
                // Окно может быть свёрнуто в трей: без уведомления звонок
                // просто не заметят — подсветка панели задач его не заменит.
                #[cfg(windows)]
                crate::tray::notify("Входящий звонок", &format!("{nick} зовёт в эфир"));
            }
        } else {
            self.call_attention = false;
        }

        // Трей: «крестик» прячет окно, а не закрывает приложение — рация должна
        // принимать звонки, пока окна нет. Выйти можно из меню иконки.
        #[cfg(windows)]
        {
            if crate::tray::take_quit() {
                self.tray_quit = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            if crate::tray::take_show() {
                self.hidden_to_tray = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            let root = ctx.viewport_id() == egui::ViewportId::ROOT;
            let wants_close =
                root && !self.tray_quit && ctx.input(|i| i.viewport().close_requested());
            if wants_close && self.config.close_to_tray.unwrap_or(true) {
                {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                    self.hidden_to_tray = true;
                    crate::tray::notify(
                        "Albus свёрнут в трей",
                        "Приложение работает: эфир и звонки продолжаются",
                    );
                }
            }
            if self.hidden_to_tray {
                // Скрытое окно просыпается только по запросу: без этого опроса
                // команда из меню иконки ждала бы следующего события.
                ctx.request_repaint_after(Duration::from_millis(200));
            }
        }

        #[cfg(windows)]
        self.apply_dark_titlebar(frame);

        #[cfg(windows)]
        {
            // свои кадры: показать и отправить зрителям
            if let Some(video) = &mut self.video {
                let units = video.poll(ctx);
                if let Some(link) = &self.video_link {
                    for unit in &units {
                        link.publish(unit, video.info());
                    }
                }
            }

            if let Some(link) = &self.video_link {
                link.set_buffer_ms(
                    self.config
                        .video_buffer_ms
                        .unwrap_or(crate::network::video_link::DEFAULT_BUFFER_MS),
                );

                // Своя трансляция возобновляет анонс, если он потерялся:
                // «Выйти из эфира» снимает его, а видео при этом не гаснет.
                if self.video.is_some() && !self.room.is_left() {
                    if let Some(dir) = &self.video_directory {
                        if dir.my_gen() == 0 {
                            link.begin_stream();
                        }
                    }
                }

                // Маршрут по комнате: живой поток, хост и свои флаги.
                // Топология звездой — издатель отдаёт поток хосту, зрители
                // идут к хосту; показ и свой feed считаются от одного набора.
                let left = self.room.is_left();
                let my_gen = self
                    .video_directory
                    .as_ref()
                    .map(|dir| dir.my_gen())
                    .unwrap_or(0);
                let is_host = self.is_host.load(std::sync::atomic::Ordering::Relaxed);
                let live: Vec<(String, u32)> = match (&self.video_directory, left) {
                    (Some(dir), false) => dir
                        .live(Instant::now())
                        .into_iter()
                        .filter(|(node, _)| *node != self.my_node_id)
                        .collect(),
                    _ => Vec::new(),
                };
                let host = if left {
                    None
                } else {
                    self.room.host_peer().map(|peer| peer.address)
                };
                // Вещающий хост релеем быть не может: его окно занято своим
                // потоком. Живой анонс хоста — признак того, что он вещает.
                let host_busy = host
                    .as_deref()
                    .map(|h| live.iter().any(|(node, _)| node == h))
                    .unwrap_or(false);
                let host_ref = host.as_deref();

                let route = crate::network::video_link::choose_view_route(
                    is_host, left, my_gen, &live, host_ref, host_busy,
                );
                link.sync_viewing(route.target());
                let feed_target = crate::network::video_link::choose_feed_target(
                    is_host, my_gen, &live, host_ref, host_busy,
                );
                link.sync_feed(feed_target);

                // Источник пропал или сменился — старое окно показывать нечего.
                // Сброс очереди (обрыв, переподключение) тоже гасит картинку:
                // последний кадр прошлой подписки иначе висел бы на экране.
                let stale_peer = match (&self.remote, route.expected_peer()) {
                    (Some(remote), Some(peer)) => peer != remote.peer,
                    (Some(_), None) => true,
                    (None, _) => false,
                };
                let stale_epoch = link.remote_epoch() != self.remote_epoch_seen;
                if stale_peer || stale_epoch {
                    self.remote = None;
                }
                self.remote_epoch_seen = link.remote_epoch();
            }

            // чужие кадры из сети; за один апдейт не больше нескольких,
            // чтобы очередь декодирования не растягивала кадр GUI
            if let Some(link) = &self.video_link {
                let mut budget = 4;
                while budget > 0 {
                    let Some(frame) = link.try_recv_due() else {
                        break;
                    };
                    budget -= 1;
                    if self.remote.is_none() {
                        let info = crate::video::StreamInfo {
                            width: frame.width,
                            height: frame.height,
                            fps: frame.fps,
                            sequence_header: frame.sequence_header.clone(),
                        };
                        match video_session::RemoteView::new(&info, frame.from.clone()) {
                            Ok(v) => {
                                self.remote = Some(v);
                                // зрителю не нужно искать кнопку — сразу открываем окно
                                self.video_window = true;
                            }
                            Err(e) => self.video_error = Some(e),
                        }
                    }
                    if let Some(remote) = &mut self.remote {
                        remote.feed(frame.unit, ctx);
                    }
                    // счётчик для отчёта: сколько кадров реально принято
                    self.remote_frames += 1;
                }
            }
            // забрать готовый кадр, даже если новых не пришло
            if let Some(remote) = &mut self.remote {
                remote.tick(ctx);
            }
        }

        // Сначала подтягиваем список устройств (карту могли подключить или
        // выдернуть), затем сверяем выбор: индексы уже пересчитаны по именам.
        self.refresh_devices();
        self.reconcile_inputs();

        let current_output_index = self
            .selected_output
            .load(std::sync::atomic::Ordering::Relaxed);
        let current_output_name = self
            .output_devices
            .get(current_output_index as usize)
            .cloned();
        if current_output_name != self.prev_output_name {
            if let Some(device_name) = current_output_name.clone() {
                // Ошибку показываем, а не только пишем в консоль: иначе
                // «выбрал другие динамики» выглядит как «звук пропал».
                if let Err(e) = stream_control::restart_playback_stream(
                    &self.playback_stream,
                    self.playback_rx.clone(),
                    device_name.clone(),
                    self.mixer.clone(),
                    self.diagnostics.clone(),
                ) {
                    self.show_toast(&format!("Динамики не переключились: {e}"));
                }
                self.config.output_device = Some(device_name);
                self.pending_save = true;
            }
            self.prev_output_name = current_output_name;
        }

        crate::keybind::check_ptt_status(
            ctx,
            &self.ptt_flag,
            self.ptt_bind,
            self.continuous_mode,
        );

        let transmitting = self.ptt_flag.load(std::sync::atomic::Ordering::Relaxed);
        let level = self
            .mixer
            .input1_peak()
            .max(self.mixer.input2_peak())
            .clamp(0.0, 1.0);

        self.sample_diagnostics();

        self.wave.push_back(level);
        while self.wave.len() > 56 {
            self.wave.pop_front();
        }
        let wave: Vec<f32> = self.wave.iter().copied().collect();

        egui::TopBottomPanel::top("top_bar")
            .exact_height(54.0)
            .frame(
                egui::Frame::none()
                    .fill(t::BG)
                    .inner_margin(egui::Margin::symmetric(22.0, 10.0)),
            )
            .show(ctx, |ui| {
                let mut copy = false;
                crate::widgets::info_panel::draw_top_bar(
                    ui,
                    &self.my_node_id,
                    &self.nick,
                    &mut self.settings_open,
                    &mut copy,
                );
                if copy {
                    self.copy_node_id(ctx);
                }
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(t::BG)
                    .inner_margin(egui::Margin {
                        left: 18.0,
                        right: 18.0,
                        top: 4.0,
                        bottom: 12.0,
                    }),
            )
            .show(ctx, |ui| {
                if let Some(err) = self.network_error.clone() {
                    egui::Frame::none()
                        .fill(t::with_alpha(t::REC, 0.10))
                        .rounding(Rounding::same(8.0))
                        .stroke(Stroke::new(1.0_f32, t::with_alpha(t::REC, 0.30)))
                        .inner_margin(egui::Margin::symmetric(10.0, 6.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(egui::RichText::new(err).size(10.5).color(t::REC));
                        });
                    ui.add_space(6.0);
                }

                let wide = ui.available_width() >= 900.0;
                if wide {
                    let rail_w = 330.0;
                    let gap = 12.0;
                    let left_w = (ui.available_width() - rail_w - gap).max(400.0);
                    let height = ui.available_height();
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        ui.allocate_ui_with_layout(
                            egui::vec2(left_w, height),
                            Layout::top_down(Align::Min),
                            |ui| {
                                self.draw_console(ui, level, transmitting, &wave);
                            },
                        );
                        ui.add_space(gap);
                        ui.allocate_ui_with_layout(
                            egui::vec2(rail_w, height),
                            Layout::top_down(Align::Min),
                            |ui| {
                                self.draw_rail(ui);
                            },
                        );
                    });
                } else {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            self.draw_console(ui, level, transmitting, &wave);
                            ui.add_space(10.0);
                            self.draw_rail(ui);
                            ui.add_space(10.0);
                        });
                }
            });

        // одновременно открыт только один оверлей
        if self.diag_open && (self.settings_open || self.chat_open) {
            self.settings_open = false;
            self.chat_open = false;
        }
        if self.settings_open && self.chat_open {
            self.chat_open = false;
        }
        if self.settings_open {
            self.draw_settings_overlay(ctx);
        }
        if self.chat_open {
            self.draw_chat_overlay(ctx);
        }
        if self.diag_open {
            self.draw_diagnostics(ctx);
        }
        self.draw_call_overlay(ctx);
        self.draw_toast(ctx);

        #[cfg(windows)]
        self.draw_video_window(ctx);

        self.persist_if_needed();

        ctx.request_repaint();
    }
}

impl RadioApp {
    fn draw_console(&mut self, ui: &mut egui::Ui, level: f32, transmitting: bool, wave: &[f32]) {
        let peers = self.room.get_all_peers();
        let is_host = self.is_host.load(std::sync::atomic::Ordering::Relaxed);

        let total_h = (ui.available_height() - 2.0).max(420.0);
        let mut open_chat = false;
        let mut open_diagnostics = false;

        egui::Frame::none()
            .fill(t::SURFACE)
            .rounding(Rounding::same(14.0))
            .stroke(Stroke::new(1.0_f32, t::STROKE_SOFT))
            .inner_margin(egui::Margin::symmetric(14.0, 12.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(total_h - 24.0);

                // Заголовок эфира — внутри панели, как в макете
                ui.horizontal(|ui| {
                    let (dot, _) =
                        ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 3.5, t::LIVE);
                    ui.label(
                        egui::RichText::new("Эфир")
                            .size(14.0)
                            .strong()
                            .color(t::TEXT),
                    );
                    ui.label(
                        egui::RichText::new(members_label(peers.len() + 1))
                            .size(9.5)
                            .color(t::TEXT_MUTED),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if is_host {
                            t::badge(ui, "ХОСТ", t::GOLD);
                        }
                        // Чат — в шапке эфира: в подвале бейдж непрочитанного
                        // обрезался за нижним краем окна при размере по умолчанию.
                        ui.add_space(8.0);
                        let unread = self.unread_total();
                        if chat_chip(ui, unread, self.chat_open).clicked() {
                            open_chat = true;
                        }
                        ui.add_space(6.0);
                        // Диагностика рядом с чатом: когда «плохо слышно», её
                        // ищут в первую очередь, а не в настройках.
                        if t::chip_button(ui, "Диагностика", self.diag_open).clicked() {
                            open_diagnostics = true;
                        }
                    });
                });

                // Геометрия снизу вверх, как в макете: футер прижат к низу
                // карточки, над ним PTT, полоса линии и центральный блок.
                let wave_h = 56.0;
                let ptt_h = 54.0;
                let footer_h = 26.0;

                let full = ui.max_rect();
                let bottom = full.bottom();
                let footer_top = bottom - footer_h;
                let ptt_top = footer_top - 24.0 - ptt_h;
                let wave_top = ptt_top - 8.0 - wave_h;
                let mid_top = ui.cursor().top() + 4.0;
                let mid_bottom = (wave_top - 8.0).max(mid_top + 200.0);
                let mid_h = mid_bottom - mid_top;

                let dial_w = (full.width() * 0.46).clamp(230.0, 330.0);

                let mid_rect = egui::Rect::from_min_max(
                    egui::pos2(full.left(), mid_top),
                    egui::pos2(full.right(), mid_bottom),
                );
                ui.allocate_ui_at_rect(mid_rect, |ui| {
                    ui.horizontal_top(|ui| {
                        // Герой в макете сидит ближе к низу блока
                        let pad = (mid_h - 315.0 - 12.0).max(0.0);

                        ui.allocate_ui_with_layout(
                            egui::vec2(dial_w, mid_h),
                            Layout::top_down(Align::Center),
                            |ui| {
                                ui.add_space(pad);
                                crate::widgets::hero::draw_hero(
                                    ui,
                                    level,
                                    transmitting,
                                    is_host,
                                    wave,
                                    peers.len(),
                                    &self.nick,
                                );
                            },
                        );

                        ui.add_space(4.0);

                        ui.allocate_ui_with_layout(
                            egui::vec2(full.width() - dial_w - 20.0, mid_h),
                            Layout::top_down(Align::Min),
                            |ui| {
                                ui.add_space(pad);
                                self.draw_stream_card(ui);
                                ui.add_space(8.0);
                                self.draw_mixer_card(ui);
                            },
                        );
                    });
                });

                let wave_rect = egui::Rect::from_min_max(
                    egui::pos2(full.left(), wave_top),
                    egui::pos2(full.right(), wave_top + wave_h),
                );
                ui.allocate_ui_at_rect(wave_rect, |ui| {
                    crate::widgets::hero::draw_wave_strip(ui, wave, transmitting, wave_h);
                });

                let ptt_rect = egui::Rect::from_min_max(
                    egui::pos2(full.left(), ptt_top),
                    egui::pos2(full.right(), ptt_top + ptt_h),
                );
                ui.allocate_ui_at_rect(ptt_rect, |ui| {
                    crate::widgets::ptt_button::draw_ptt_button(
                        ui,
                        &self.ptt_flag,
                        &mut self.continuous_mode,
                        &self.mixer,
                        self.ptt_bind,
                    );
                });

                let footer_rect = egui::Rect::from_min_max(
                    egui::pos2(full.left(), footer_top),
                    egui::pos2(full.right(), bottom),
                );
                ui.allocate_ui_at_rect(footer_rect, |ui| {
                    ui.horizontal(|ui| {
                        crate::widgets::ptt_button::draw_continuous_toggle(
                            ui,
                            &self.ptt_flag,
                            &mut self.continuous_mode,
                        );
                        ui.add_space(10.0);
                        crate::widgets::ptt_button::draw_mic_mute_toggle(ui, &self.mixer);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} FPS",
                                    self.config.video_fps.unwrap_or(60)
                                ))
                                .size(9.0)
                                .monospace()
                                .color(t::TEXT_MUTED),
                            );
                        });
                    });
                });
            });

        if open_chat {
            self.open_chat();
        }
        if open_diagnostics {
            self.diag_open = !self.diag_open;
        }
    }

    /// Карточка трансляции: миниатюра слева, данные и управление справа
    #[cfg(windows)]
    fn draw_stream_card(&mut self, ui: &mut egui::Ui) {
        let running = self.video.is_some();
        let mut open_window = false;

        egui::Frame::none()
            .fill(t::INNER)
            .rounding(Rounding::same(10.0))
            .stroke(Stroke::new(1.0_f32, t::STROKE_SOFT))
            .inner_margin(egui::Margin::symmetric(9.0, 9.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.horizontal_top(|ui| {
                    let size = egui::vec2(128.0, 72.0);
                    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
                    let resp = resp
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("Открыть окно трансляции");
                    let painter = ui.painter_at(rect);
                    painter.rect_filled(rect, Rounding::same(6.0), t::INSET);
                    painter.rect_stroke(
                        rect,
                        Rounding::same(6.0),
                        Stroke::new(1.0_f32, t::STROKE_SOFT),
                    );

                    let preview = self
                        .remote
                        .as_ref()
                        .map(|r| (r.texture(), r.size()))
                        .or_else(|| self.video.as_ref().map(|v| (v.texture(), v.size())));
                    let mut has_frame = false;
                    if let Some((Some(tex), (w, h))) = preview {
                        // вписываем кадр в миниатюру, сохраняя пропорции
                        let src_ratio = w.max(1) as f32 / h.max(1) as f32;
                        let box_ratio = rect.width() / rect.height();
                        let target = if src_ratio > box_ratio {
                            egui::vec2(rect.width(), rect.width() / src_ratio)
                        } else {
                            egui::vec2(rect.height() * src_ratio, rect.height())
                        };
                        let frame = egui::Rect::from_center_size(rect.center(), target);
                        painter.image(
                            tex.id(),
                            frame,
                            egui::Rect::from_min_max(
                                egui::pos2(0.0, 0.0),
                                egui::pos2(1.0, 1.0),
                            ),
                            egui::Color32::WHITE,
                        );
                        has_frame = true;
                    }
                    if !has_frame {
                        // Заглушка окна: без активного потока превью пустое
                        let inner = rect.shrink(8.0);
                        painter.rect_filled(
                            inner,
                            Rounding::same(4.0),
                            egui::Color32::from_rgb(19, 19, 23),
                        );
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                inner.min,
                                egui::vec2(inner.width(), 6.0),
                            ),
                            Rounding::same(4.0),
                            egui::Color32::from_rgb(31, 31, 37),
                        );
                        painter.rect_filled(
                            egui::Rect::from_min_size(
                                egui::pos2(inner.right() - 26.0, inner.top() + 12.0),
                                egui::vec2(18.0, inner.height() - 20.0),
                            ),
                            Rounding::same(2.0),
                            egui::Color32::from_rgb(40, 34, 62),
                        );
                        let mut y = inner.top() + 13.0;
                        for w in [0.52_f32, 0.38, 0.46] {
                            painter.rect_filled(
                                egui::Rect::from_min_size(
                                    egui::pos2(inner.left() + 7.0, y),
                                    egui::vec2((inner.width() - 44.0) * w, 3.0),
                                ),
                                Rounding::same(1.5),
                                egui::Color32::from_rgb(38, 38, 45),
                            );
                            y += 9.0;
                        }
                    }

                    if running {
                        let pill = egui::Rect::from_min_size(
                            rect.left_top() + egui::vec2(6.0, 6.0),
                            egui::vec2(38.0, 15.0),
                        );
                        painter.rect_filled(
                            pill,
                            Rounding::same(4.0),
                            egui::Color32::from_black_alpha(170),
                        );
                        painter.circle_filled(
                            egui::pos2(pill.left() + 8.0, pill.center().y),
                            2.5,
                            t::LIVE,
                        );
                        painter.text(
                            egui::pos2(pill.left() + 14.0, pill.center().y),
                            egui::Align2::LEFT_CENTER,
                            "LIVE",
                            egui::FontId::proportional(7.5),
                            t::TEXT,
                        );
                    }
                    if resp.clicked() {
                        open_window = true;
                    }

                    ui.add_space(10.0);

                    ui.vertical(|ui| {
                        ui.set_width(ui.available_width());

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("ТРАНСЛЯЦИЯ")
                                    .size(8.5)
                                    .strong()
                                    .color(t::TEXT_MUTED),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let fps = self
                                    .video
                                    .as_ref()
                                    .map(|v| v.fps().round() as u32)
                                    .unwrap_or_else(|| match self.config.video_fps {
                                        Some(30) => 30,
                                        _ => 60,
                                    });
                                let label = ui.label(
                                    egui::RichText::new(format!("{} FPS", fps))
                                        .size(8.0)
                                        .monospace()
                                        .color(t::TEXT_MUTED),
                                );
                                if let Some(v) = &self.video {
                                    let (d, r, i) = v.timings();
                                    label.on_hover_text(format!(
                                        "захват {} FPS\nдекод {:.1} · чтение GPU {:.1} · картинка {:.1} мс",
                                        v.encode_fps(),
                                        d,
                                        r,
                                        i
                                    ));
                                }
                            });
                        });

                        let title = self
                            .video
                            .as_ref()
                            .map(|v| v.source.clone())
                            .or_else(|| self.remote.as_ref().map(|r| format!("Смотрит: {}", r.peer)))
                            .unwrap_or_else(|| "Экран 1".to_string());
                        ui.label(
                            egui::RichText::new(title)
                                .size(12.0)
                                .strong()
                                .color(if running { t::TEXT } else { t::TEXT_DIM }),
                        );

                        let subtitle = if let Some(v) = &self.video {
                            let (w, h) = v.size();
                            let viewers = self
                                .video_link
                                .as_ref()
                                .map(|l| l.viewer_count())
                                .unwrap_or(0);
                            format!(
                                "{}×{} · {} FPS · {} зрит.",
                                w,
                                h,
                                v.encode_fps(),
                                viewers
                            )
                        } else {
                            "Не запущена".to_string()
                        };
                        ui.label(
                            egui::RichText::new(subtitle)
                                .size(8.0)
                                .color(t::TEXT_MUTED),
                        );

                        ui.add_space(6.0);

                        ui.horizontal(|ui| {
                            let label = if running { "Стоп" } else { "Запустить" };
                            if t::chip_button(ui, label, running).clicked() {
                                if running {
                                    self.video = None;
                                    self.video_fullscreen = false;
                                    if let Some(link) = &self.video_link {
                                        link.end_stream();
                                    }
                                } else {
                                    let fps = match self.config.video_fps {
                                        Some(30) => 30,
                                        _ => 60,
                                    };
                                    match video_session::VideoSession::start(fps) {
                                        Ok(v) => {
                                            self.video = Some(v);
                                            self.video_error = None;
                                            // Анонс новой трансляции: зрители
                                            // найдут её в каталоге и подпишутся
                                            // сами, в том числе вошедшие позже.
                                            if let Some(link) = &self.video_link {
                                                link.begin_stream();
                                            }
                                        }
                                        Err(e) => self.video_error = Some(e),
                                    }
                                }
                            }

                            if self.remote.is_some()
                                && t::chip_button(ui, "Смотреть", false).clicked()
                            {
                                open_window = true;
                            }

                            if let Some(err) = self.video_error.clone() {
                                ui.label(egui::RichText::new(err).size(8.0).color(t::REC));
                            }
                        });
                    });
                });
            });

        if open_window {
            self.video_window = true;
        }
    }

    #[cfg(not(windows))]
    fn draw_stream_card(&mut self, _ui: &mut egui::Ui) {}

    /// Микшер: микрофон, гитара и громкость собеседников
    fn draw_mixer_card(&mut self, ui: &mut egui::Ui) {
        egui::Frame::none()
            .fill(t::INNER)
            .rounding(Rounding::same(10.0))
            .stroke(Stroke::new(1.0_f32, t::STROKE_SOFT))
            .inner_margin(egui::Margin::symmetric(10.0, 8.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("ГРОМКОСТЬ")
                            .size(8.5)
                            .strong()
                            .color(t::TEXT_MUTED),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        // Как в макете: значок микшера вместо подписи
                        if t::icon_button(ui, t::Icon::Sliders, 18.0, false)
                            .on_hover_text("Настройки звука")
                            .clicked()
                        {
                            self.settings_open = true;
                        }
                    });
                });
                ui.add_space(2.0);

                let mut mic = self.mixer.ch1_volume();
                let mut guitar = self.mixer.ch2_volume();
                let mut peers = self.mixer.output_volume();

                // Подпись — номер входа: профиль обработки живёт в настройках,
                // а по громкости видно, что за вход перед вами.
                mixer_row(ui, "ВХОД 1", &mut mic, self.mixer.input1_peak());
                mixer_row(ui, "ВХОД 2", &mut guitar, self.mixer.input2_peak());
                mixer_row(ui, "СОБЕСЕДНИКИ", &mut peers, 0.0);

                self.mixer.set_ch1_volume(mic);
                self.mixer.set_ch2_volume(guitar);
                self.mixer.set_output_volume(peers);
            });
    }

    fn draw_rail(&mut self, ui: &mut egui::Ui) {
        // Как в макете: одна карточка на весь рейл, секции — волосяными
        // линиями во всю ширину, подвал прижат к низу внутри карточки.
        let footer_h = 28.0;
        let mut do_leave = false;

        egui::Frame::none()
            .fill(t::SURFACE)
            .rounding(Rounding::same(t::RADIUS))
            .stroke(Stroke::new(1.0_f32, t::STROKE_SOFT))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let scroll_h = (ui.available_height() - footer_h).max(240.0);

                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .max_height(scroll_h)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());

                        let mut copy = false;
                        rail_section(ui, |ui| {
                            if crate::widgets::room_panel::draw_room_connect(
                                ui,
                                &mut self.connect_ip_input,
                                self.iroh.clone(),
                                self.my_id,
                                self.is_host.clone(),
                                self.room.clone(),
                                &self.calls,
                            ) {
                                do_leave = true;
                            }
                        });
                        rail_divider(ui);

                        rail_section(ui, |ui| {
                            copy = crate::widgets::room_panel::draw_my_nodeid(
                                ui,
                                &self.my_node_id,
                            );
                        });
                        if copy {
                            self.copy_node_id(ui.ctx());
                        }
                        rail_divider(ui);

                        rail_section(ui, |ui| {
                            crate::widgets::room_panel::draw_participants(
                                ui,
                                self.room.clone(),
                                &self.mixer,
                                self.is_host.clone(),
                                self.ptt_flag.load(std::sync::atomic::Ordering::Relaxed),
                                &self.nick,
                                &self.friends,
                            );
                        });
                        rail_divider(ui);

                        let mut offline_toast: Option<String> = None;
                        rail_section(ui, |ui| {
                            crate::widgets::friends_panel::draw_friends_panel(
                                ui,
                                &self.friends,
                                self.iroh.clone(),
                                self.my_id,
                                &self.my_node_id,
                                &self.nick,
                                &mut self.add_friend_node,
                                &self.chat_log,
                                &mut self.active_chat,
                                &mut self.chat_open,
                                &self.calls,
                                &self.presence,
                                &mut offline_toast,
                                &self.room,
                                &self.is_host,
                            );
                        });
                        if let Some(text) = offline_toast {
                            self.show_toast(&text);
                        }
                        if self.chat_open {
                            self.settings_open = false;
                        }
                    });

                // Подвал внутри карточки
                rail_divider(ui);
                ui.add_space(2.0);
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    let (dot, _) =
                        ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
                    ui.painter().circle_filled(dot.center(), 2.5, t::LIVE);
                    ui.add_space(6.0);
                    t::micro(ui, "Сквозное шифрование");
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_space(14.0);
                        t::micro(ui, "v0.1");
                    });
                });
            });

        if do_leave {
            self.leave_voice();
        }
    }

    /// «Выйти из эфира»: закрыть звонок, отпустить всех собеседников и не
    /// пускать их обратно, пока не нажмут «Войти» или не позвонят снова.
    fn leave_voice(&mut self) {
        // Объявляем уход до того, как разберём фазу звонка. Комната может быть
        // собрана одним mesh (без CallPhase::Active), и тогда «Выйти из эфира»
        // ничего не отправляло: собеседники держали нас в списке «молчит» до
        // таймаута и не могли пригласить обратно.
        self.calls.announce_leave();
        match self.calls.view() {
            crate::calls::CallView::Active { .. } => self.calls.end_active(),
            crate::calls::CallView::Outgoing { .. } => self.calls.cancel_outgoing(),
            crate::calls::CallView::Incoming { .. } => {
                self.calls.decline_incoming();
            }
            crate::calls::CallView::Idle => {}
        }
        self.mixer.set_ring_mode(self.calls.ring_mode());
        // Флаг «вышли» ставится до обрыва соединений: иначе их последний
        // IrohSync ещё успел бы вернуть собеседника в комнату.
        self.room.leave();
        // Видео вне комнаты не живёт: своя трансляция снимается с анонса,
        // чужая — снимается подписка и очищается очередь.
        #[cfg(windows)]
        if let Some(link) = &self.video_link {
            link.end_stream();
            link.unsubscribe();
        }
        #[cfg(windows)]
        {
            self.remote = None;
        }
        self.is_host.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(p2p) = self.iroh.clone() {
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                // Отказ/отмена/завершение шлются отдельной таской — даём им
                // уйти до обрыва, иначе собеседник ждёт таймаута вместо
                // мгновенного «звонок отклонён/завершён».
                handle.spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
                    p2p.disconnect_all().await;
                });
            }
        }
        self.show_toast("Вы вышли из эфира");
    }

    /// Панель «Диагностика»: сеть, звук, видео и кнопка «Скопировать отчёт».
    ///
    /// Числа берутся из того же `ReportInput`, что уходит в буфер обмена, —
    /// экран и отчёт не расходятся. Панель не управляет звуком и видео: это
    /// только наблюдение, поэтому и прав тут нет.
    fn draw_diagnostics(&mut self, ctx: &egui::Context) {
        let input = self.diagnostics_input();
        let screen = ctx.screen_rect();
        let mut close = false;
        let mut copy = false;

        let scrim = egui::Area::new(egui::Id::new("diag_scrim"))
            .fixed_pos(screen.min)
            .order(Order::Middle)
            .show(ctx, |ui| {
                let (rect, resp) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
                ui.painter()
                    .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(150));
                resp
            })
            .inner;
        if scrim.clicked() {
            close = true;
        }

        let sheet_w = 620.0_f32.min(screen.width() - 60.0);
        let sheet_h = 560.0_f32.min(screen.height() - 70.0);
        let pos = egui::pos2(
            screen.center().x - sheet_w * 0.5,
            screen.center().y - sheet_h * 0.5,
        );

        egui::Area::new(egui::Id::new("diag_sheet"))
            .fixed_pos(pos)
            .order(Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(t::SURFACE)
                    .rounding(Rounding::same(16.0))
                    .stroke(Stroke::new(1.0_f32, t::STROKE))
                    .shadow(egui::epaint::Shadow {
                        offset: egui::vec2(0.0, 18.0),
                        blur: 50.0,
                        spread: 0.0,
                        color: egui::Color32::from_black_alpha(170),
                    })
                    .inner_margin(egui::Margin::symmetric(18.0, 14.0))
                    .show(ui, |ui| {
                        ui.set_width(sheet_w - 36.0);
                        ui.set_height(sheet_h - 28.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Диагностика")
                                    .size(16.0)
                                    .strong()
                                    .color(t::TEXT),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new("наблюдение, ничего не меняет")
                                    .size(9.5)
                                    .color(t::TEXT_MUTED),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if t::icon_button(ui, t::Icon::Close, 26.0, false)
                                    .on_hover_text("Закрыть")
                                    .clicked()
                                {
                                    close = true;
                                }
                            });
                        });

                        ui.add_space(4.0);
                        t::divider(ui);
                        ui.add_space(6.0);

                        egui::ScrollArea::vertical()
                            .max_height(sheet_h - 132.0)
                            .auto_shrink([false; 2])
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                draw_diagnostics_body(ui, &input);
                            });

                        ui.add_space(6.0);
                        t::divider(ui);
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if t::primary_button(
                                ui,
                                "Скопировать отчёт",
                                egui::vec2(180.0, 30.0),
                            )
                            .clicked()
                            {
                                copy = true;
                            }
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new(
                                    "отчёт можно вставить в чат: у собеседника будут те же числа",
                                )
                                .size(9.5)
                                .color(t::TEXT_MUTED),
                            );
                        });
                    });
            });

        if copy {
            self.copy_diagnostics(ctx);
        }
        // Esc закрывает панель, как и клик по затемнению
        if close || (!ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Escape)))
        {
            self.diag_open = false;
        }
    }

    fn draw_settings_overlay(&mut self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();
        let mut close = false;
        let mut copy = false;

        let scrim = egui::Area::new(egui::Id::new("settings_scrim"))
            .fixed_pos(screen.min)
            .order(Order::Middle)
            .show(ctx, |ui| {
                let (rect, resp) =
                    ui.allocate_exact_size(screen.size(), egui::Sense::click());
                ui.painter()
                    .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(150));
                resp
            })
            .inner;
        if scrim.clicked() {
            close = true;
        }

        let sheet_w = 690.0_f32.min(screen.width() - 60.0);
        let sheet_h = 540.0_f32.min(screen.height() - 70.0);
        let pos = egui::pos2(
            screen.center().x - sheet_w * 0.5,
            screen.center().y - sheet_h * 0.5,
        );

        egui::Area::new(egui::Id::new("settings_sheet"))
            .fixed_pos(pos)
            .order(Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(t::SURFACE)
                    .rounding(Rounding::same(16.0))
                    .stroke(Stroke::new(1.0_f32, t::STROKE))
                    .shadow(egui::epaint::Shadow {
                        offset: egui::vec2(0.0, 18.0),
                        blur: 50.0,
                        spread: 0.0,
                        color: egui::Color32::from_black_alpha(170),
                    })
                    .inner_margin(egui::Margin::symmetric(18.0, 14.0))
                    .show(ui, |ui| {
                        ui.set_width(sheet_w - 36.0);
                        ui.set_height(sheet_h - 28.0);

                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Настройки")
                                    .size(16.0)
                                    .strong()
                                    .color(t::TEXT),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if t::icon_button(ui, t::Icon::Close, 26.0, false)
                                    .on_hover_text("Закрыть")
                                    .clicked()
                                {
                                    close = true;
                                }
                            });
                        });

                        ui.add_space(4.0);
                        t::divider(ui);
                        ui.add_space(6.0);

                        egui::ScrollArea::vertical()
                            .max_height(sheet_h - 90.0)
                            .auto_shrink([false; 2])
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                crate::widgets::audio_settings::draw_audio_settings(
                                    ui,
                                    &self.input_devices,
                                    &self.input_channel_counts,
                                    &self.output_devices,
                                    &self.selected_input,
                                    &self.selected_input2,
                                    self.input2_error.as_deref(),
                                    &self.selected_output,
                                    &self.mixer,
                                    &mut self.ptt_bind,
                                );

                                ui.add_space(14.0);
                                t::divider(ui);
                                ui.add_space(8.0);

                                t::section_label(ui, "ЭКРАН");
                                ui.add_space(4.0);
                                let fps = match self.config.video_fps {
                                    Some(30) => 30,
                                    _ => 60,
                                };
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("Кадров в секунду")
                                            .size(10.5)
                                            .color(t::TEXT_MUTED),
                                    );
                                    ui.add_space(8.0);
                                    if t::chip_button(ui, "30", fps == 30).clicked() {
                                        self.config.video_fps = Some(30);
                                        self.pending_save = true;
                                    }
                                    if t::chip_button(ui, "60", fps == 60).clicked() {
                                        self.config.video_fps = Some(60);
                                        self.pending_save = true;
                                    }
                                });

                                ui.add_space(8.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("Буфер показа")
                                            .size(10.5)
                                            .color(t::TEXT_MUTED),
                                    );
                                    ui.add_space(8.0);
                                    let mut buffer = self.config.video_buffer_ms.unwrap_or(
                                        crate::network::video_link::DEFAULT_BUFFER_MS,
                                    );
                                    let changed = ui
                                        .add(
                                            egui::Slider::new(&mut buffer, 0..=5000u32)
                                                .step_by(100.0)
                                                .suffix(" мс"),
                                        )
                                        .changed();
                                    if changed {
                                        self.config.video_buffer_ms = Some(buffer);
                                        self.pending_save = true;
                                        if let Some(link) = &self.video_link {
                                            link.set_buffer_ms(buffer);
                                        }
                                    }
                                });
                                ui.label(
                                    egui::RichText::new(
                                        "Буфер зрителя: сеть успевает отдать отставший кадр. 0 — показывать сразу.",
                                    )
                                    .size(8.5)
                                    .color(t::TEXT_MUTED),
                                );

                                ui.add_space(14.0);
                                t::divider(ui);
                                ui.add_space(8.0);

                                t::section_label(ui, "СИСТЕМА");
                                ui.add_space(4.0);
                                let mut close_to_tray =
                                    self.config.close_to_tray.unwrap_or(true);
                                if ui
                                    .checkbox(
                                        &mut close_to_tray,
                                        egui::RichText::new(
                                            "Закрывать в трей, не выходя из эфира",
                                        )
                                        .size(10.5)
                                        .color(t::TEXT_MUTED),
                                    )
                                    .changed()
                                {
                                    self.config.close_to_tray = Some(close_to_tray);
                                    self.pending_save = true;
                                }
                                ui.add_space(4.0);
                                let mut autostart = self.autostart_on;
                                if ui
                                    .checkbox(
                                        &mut autostart,
                                        egui::RichText::new("Запускать при входе в Windows")
                                            .size(10.5)
                                            .color(t::TEXT_MUTED),
                                    )
                                    .changed()
                                {
                                    if crate::autostart::set_enabled(autostart) {
                                        self.autostart_on = autostart;
                                        self.show_toast(if autostart {
                                            "Автозапуск включён"
                                        } else {
                                            "Автозапуск выключен"
                                        });
                                    } else {
                                        self.show_toast("Не удалось изменить автозапуск");
                                    }
                                }
                                ui.label(
                                    egui::RichText::new(
                                        "Трей: двойной клик по иконке открывает окно, меню — выход.",
                                    )
                                    .size(8.5)
                                    .color(t::TEXT_MUTED),
                                );

                                ui.add_space(14.0);
                                t::divider(ui);
                                ui.add_space(8.0);

                                t::section_label(ui, "ЛИЧНОСТЬ");
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new("Ваше имя")
                                            .size(10.5)
                                            .color(t::TEXT_MUTED),
                                    );
                                    ui.add_space(8.0);
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.nick)
                                            .desired_width(180.0),
                                    );
                                });

                                ui.add_space(6.0);
                                t::inset_frame().show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(&self.my_node_id)
                                                .monospace()
                                                .size(9.0)
                                                .color(t::TEXT_MUTED),
                                        );
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if t::chip_button(ui, "Копировать", false)
                                                    .clicked()
                                                {
                                                    copy = true;
                                                }
                                            },
                                        );
                                    });
                                });

                                ui.add_space(8.0);
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new("Сбросить личность")
                                                .size(10.5)
                                                .strong()
                                                .color(t::REC),
                                        )
                                        .rounding(Rounding::same(8.0)),
                                    )
                                    .clicked()
                                {
                                    crate::identity::Identity::reset();
                                    self.reset_note = Some(
                                        "Новый NodeID создан. Перезапустите приложение."
                                            .to_string(),
                                    );
                                }
                                if let Some(note) = &self.reset_note {
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(note)
                                            .size(9.5)
                                            .color(t::TEXT_MUTED),
                                    );
                                }
                            });
                    });
            });

        if copy {
            self.copy_node_id(ctx);
        }
        if close {
            self.settings_open = false;
        }
    }

    fn draw_chat_overlay(&mut self, ctx: &egui::Context) {
        if !self.chat_open {
            return;
        }
        let room_alive = !self.room.get_all_peers().is_empty() && !self.room.is_left();
        // Вкладка «Комната» при пустом эфире бессмысленна: назад в личку.
        if self.chat_room && !room_alive && self.active_chat.is_some() {
            self.chat_room = false;
        }
        let node_id = if self.chat_room {
            String::new()
        } else {
            match self.active_chat.clone() {
                Some(id) => id,
                None => {
                    self.chat_open = false;
                    return;
                }
            }
        };
        let show_room = self.chat_room;
        let room_unread = self.room_chat.unread();
        let room_id = self.calls.room_id().unwrap_or(0);
        let mut want_room: Option<bool> = None;
        let screen = ctx.screen_rect();
        let mut close = false;

        let scrim = egui::Area::new(egui::Id::new("chat_scrim"))
            .fixed_pos(screen.min)
            .order(Order::Middle)
            .show(ctx, |ui| {
                let (rect, resp) =
                    ui.allocate_exact_size(screen.size(), egui::Sense::click());
                ui.painter()
                    .rect_filled(rect, 0.0, egui::Color32::from_black_alpha(80));
                resp
            })
            .inner;
        if scrim.clicked() {
            close = true;
        }

        let width = 392.0_f32.min(screen.width() - 20.0);
        let height = screen.height() - 20.0;
        let pos = egui::pos2(screen.right() - width - 10.0, screen.top() + 10.0);

        egui::Area::new(egui::Id::new("chat_drawer"))
            .fixed_pos(pos)
            .order(Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(t::SURFACE)
                    .rounding(Rounding::same(16.0))
                    .stroke(Stroke::new(1.0_f32, t::STROKE))
                    .shadow(egui::epaint::Shadow {
                        offset: egui::vec2(0.0, 18.0),
                        blur: 50.0,
                        spread: 0.0,
                        color: egui::Color32::from_black_alpha(160),
                    })
                    .inner_margin(egui::Margin::symmetric(14.0, 12.0))
                    .show(ui, |ui| {
                        ui.set_width(width - 28.0);
                        ui.set_height(height - 24.0);
                        // Вкладки: личка другу и общий чат комнаты. Без них
                        // «Чат» открывал только переписку, и написать в эфир
                        // было нечем.
                        ui.horizontal(|ui| {
                            if t::chip_button(ui, "Личка", !show_room).clicked() {
                                want_room = Some(false);
                            }
                            let label = if room_unread > 0 {
                                format!("Комната · {room_unread}")
                            } else {
                                "Комната".to_string()
                            };
                            let resp = t::chip_button(ui, &label, show_room);
                            if resp.clicked() && room_alive {
                                want_room = Some(true);
                            }
                            if !room_alive {
                                resp.on_hover_text("В эфире никого нет");
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if t::icon_button(ui, t::Icon::Close, 24.0, false)
                                    .on_hover_text("Закрыть")
                                    .clicked()
                                {
                                    close = true;
                                }
                            });
                        });
                        ui.add_space(4.0);
                        t::divider(ui);
                        ui.add_space(3.0);
                        if show_room {
                            crate::widgets::chat_panel::draw_room_chat_panel(
                                ui,
                                &self.room_chat,
                                &self.nick,
                                self.my_id,
                                &self.room,
                                room_id,
                                self.iroh.clone(),
                                &mut self.chat_input,
                                &mut close,
                                &self.chat_error,
                            );
                        } else {
                            crate::widgets::chat_panel::draw_chat_panel(
                                ui,
                                &self.chat_log,
                                &self.friends,
                                &node_id,
                                &mut self.chat_input,
                                self.iroh.clone(),
                                self.my_id,
                                &mut close,
                                &self.chat_error,
                                &self.presence,
                            );
                        }
                    });
            });

        if let Some(room) = want_room {
            if self.chat_room != room {
                self.chat_input.clear();
            }
            self.chat_room = room;
        }

        // Открытый чат считается прочитанным: раньше счётчик копился вечно,
        // и бейдж горел даже при уже просмотренной переписке.
        if self.chat_room {
            self.room_chat.mark_read();
        } else {
            self.chat_log.mark_read(&node_id);
            self.seen_unread.insert(node_id.clone(), 0);
        }

        if close {
            self.chat_open = false;
            self.chat_input.clear();
        }
    }

    /// Карточка звонка поверх окна: входящий, «звоним…» или идущий разговор
    fn draw_call_overlay(&mut self, ctx: &egui::Context) {
        let view = self.calls.view();
        if matches!(view, crate::calls::CallView::Idle) {
            return;
        }

        let screen = ctx.screen_rect();
        let width = 330.0_f32.min(screen.width() - 40.0);
        let pos = egui::pos2(screen.center().x - width * 0.5, screen.top() + 66.0);

        let mut accept = false;
        let mut decline = false;
        let mut cancel = false;
        let mut end = false;

        // Esc работает как крупная кнопка: выйти из звонка, не целясь мышью.
        // Но только когда фокус не в текстовом поле: там Esc — для текста,
        // иначе он заодно обрывал бы разговор при правке сообщения.
        // Enter — войти в комнату, не целясь мышью: приглашение часто приходит,
        // когда окно не в фокусе, а Esc рядом отклоняет.
        if !ctx.wants_keyboard_input()
            && matches!(view, crate::calls::CallView::Incoming { .. })
            && ctx.input(|i| i.key_pressed(egui::Key::Enter))
        {
            println!("[КОМНАТА] ENTER — входим по приглашению");
            accept = true;
        }
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            match view {
                crate::calls::CallView::Incoming { .. } => decline = true,
                crate::calls::CallView::Outgoing { .. } => cancel = true,
                crate::calls::CallView::Active { .. } => end = true,
                crate::calls::CallView::Idle => {}
            }
        }

        if let crate::calls::CallView::Active {
            members,
            peer,
            elapsed,
        } = &view
        {
            // В комнате на N подписью служит число участников; имя показываем,
            // когда собеседник один — как в прежнем личном звонке.
            let label = match (members, peer) {
                (2, Some(node)) => self
                    .friends
                    .friends()
                    .into_iter()
                    .find(|f| &f.node_id == node)
                    .map(|f| f.name)
                    .unwrap_or_else(|| crate::diagnostics::short_id(node)),
                _ => format!("{members} в комнате"),
            };
            // Идёт разговор: никакой карточки поверх окна — стрим и комната
            // должны оставаться видны. Только компактная плашка в углу.
            egui::Area::new(egui::Id::new("call_pill"))
                .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-12.0, 66.0))
                .order(Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::none()
                        .fill(t::SURFACE)
                        .rounding(Rounding::same(11.0))
                        .stroke(Stroke::new(1.0_f32, t::with_alpha(t::LIVE, 0.35)))
                        .shadow(egui::epaint::Shadow {
                            offset: egui::vec2(0.0, 8.0),
                            blur: 22.0,
                            spread: 0.0,
                            color: egui::Color32::from_black_alpha(130),
                        })
                        .inner_margin(egui::Margin::symmetric(10.0, 6.0))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let (dot, _) = ui.allocate_exact_size(
                                    egui::vec2(7.0, 7.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().circle_filled(dot.center(), 3.0, t::LIVE);
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(&label)
                                        .size(11.5)
                                        .strong()
                                        .color(t::TEXT),
                                );
                                ui.add_space(6.0);
                                ui.label(
                                    egui::RichText::new(format_elapsed(*elapsed))
                                        .size(10.0)
                                        .monospace()
                                        .color(t::TEXT_MUTED),
                                );
                                ui.add_space(8.0);
                                if ui
                                    .add(
                                        egui::Button::new(
                                            egui::RichText::new("Выйти")
                                                .size(10.0)
                                                .strong()
                                                .color(egui::Color32::WHITE),
                                        )
                                        .fill(t::REC)
                                        .rounding(Rounding::same(7.0)),
                                    )
                                    .on_hover_text("Выйти из комнаты (ESC)")
                                    .clicked()
                                {
                                    end = true;
                                }
                            });
                        });
                });
        } else {
        egui::Area::new(egui::Id::new("call_card"))
            .fixed_pos(pos)
            .order(Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(t::SURFACE)
                    .rounding(Rounding::same(14.0))
                    .stroke(Stroke::new(1.0_f32, t::with_alpha(t::BRAND, 0.45)))
                    .shadow(egui::epaint::Shadow {
                        offset: egui::vec2(0.0, 16.0),
                        blur: 42.0,
                        spread: 0.0,
                        color: egui::Color32::from_black_alpha(160),
                    })
                    .inner_margin(egui::Margin::symmetric(16.0, 12.0))
                    .show(ui, |ui| {
                        ui.set_width(width - 32.0);

                        match &view {
                            crate::calls::CallView::Idle => {}
                            crate::calls::CallView::Incoming { nick, .. } => {
                                ui.horizontal(|ui| {
                                    t::avatar(ui, nick, 34.0, true);
                                    ui.add_space(4.0);
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new("Приглашение в комнату")
                                                .size(9.0)
                                                .strong()
                                                .color(t::TEXT_MUTED),
                                        );
                                        ui.label(
                                            egui::RichText::new(nick)
                                                .size(14.0)
                                                .strong()
                                                .color(t::TEXT),
                                        );
                                    });
                                });
                                ui.add_space(10.0);
                                ui.horizontal(|ui| {
                                    if t::chip_button(ui, "Отклонить", false).clicked() {
                                        decline = true;
                                    }
                                    if t::primary_button(ui, "Войти", egui::vec2(150.0, 30.0))
                                        .clicked()
                                    {
                                        accept = true;
                                    }
                                });
                                ui.add_space(4.0);
                                t::micro(ui, "ENTER — войти · ESC — отклонить");
                            }
                            crate::calls::CallView::Outgoing { nick, elapsed } => {
                                ui.horizontal(|ui| {
                                    t::avatar(ui, nick, 34.0, false);
                                    ui.add_space(4.0);
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new("Приглашаем…")
                                                .size(9.0)
                                                .strong()
                                                .color(t::TEXT_MUTED),
                                        );
                                        ui.label(
                                            egui::RichText::new(nick)
                                                .size(14.0)
                                                .strong()
                                                .color(t::TEXT),
                                        );
                                    });
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(
                                            egui::RichText::new(format_elapsed(*elapsed))
                                                .size(10.0)
                                                .monospace()
                                                .color(t::TEXT_MUTED),
                                        );
                                    });
                                });
                                ui.add_space(10.0);
                                let w = ui.available_width();
                                if ui
                                    .add_sized(
                                        egui::vec2(w, 30.0),
                                        egui::Button::new(
                                            egui::RichText::new("Отменить приглашение")
                                                .size(11.0)
                                                .strong()
                                                .color(egui::Color32::WHITE),
                                        )
                                        .fill(t::REC)
                                        .rounding(Rounding::same(8.0)),
                                    )
                                    .clicked()
                                {
                                    cancel = true;
                                }
                                ui.add_space(4.0);
                                t::micro(ui, "ESC — отменить приглашение");
                            }
                            // Идущий разговор рисуется плашкой выше, не карточкой
                            crate::calls::CallView::Active { .. } => {}
                        }
                    });
            });
        }

        if accept {
            self.calls.accept_incoming();
        }
        if decline {
            self.calls.decline_incoming();
        }
        if cancel {
            self.calls.cancel_outgoing();
        }
        if end {
            self.calls.end_active();
        }
        if accept || decline || cancel || end {
            self.mixer
                .set_ring_mode(self.calls.ring_mode());
        }
    }

    fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some((text, at)) = self.toast.clone() else {
            return;
        };
        if at.elapsed() > Duration::from_millis(2400) {
            self.toast = None;
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(60));

        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("node_toast"))
            .fixed_pos(egui::pos2(screen.center().x, screen.bottom() - 92.0))
            .pivot(egui::Align2::CENTER_BOTTOM)
            .order(Order::Foreground)
            .show(ctx, |ui| {
                egui::Frame::none()
                    .fill(t::SURFACE_HI)
                    .rounding(Rounding::same(9.0))
                    .stroke(Stroke::new(1.0_f32, t::STROKE))
                    .shadow(egui::epaint::Shadow {
                        offset: egui::vec2(0.0, 10.0),
                        blur: 26.0,
                        spread: 0.0,
                        color: egui::Color32::from_black_alpha(150),
                    })
                    .inner_margin(egui::Margin::symmetric(12.0, 7.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (dot, _) =
                                ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                            ui.painter().circle_filled(dot.center(), 3.0, t::LIVE);
                            ui.label(egui::RichText::new(text).size(10.5).color(t::TEXT));
                        });
                    });
            });
    }

    /// Окно просмотра: отдельное, растягиваемое, с полноэкранным режимом.
    #[cfg(windows)]
    fn draw_video_window(&mut self, ctx: &egui::Context) {
        let has_source = self.video.is_some() || self.remote.is_some();
        if !self.video_window || !has_source {
            self.video_window = false;
            self.video_fullscreen = false;
            return;
        }

        let close_requested = std::cell::Cell::new(false);
        let chrome = std::cell::Cell::new(Instant::now());
        let mut builder = egui::ViewportBuilder::default()
            .with_title("Albus — трансляция")
            .with_inner_size([1280.0, 720.0])
            .with_min_inner_size([360.0, 240.0])
            .with_resizable(true);
        if self.video_fullscreen {
            builder = builder.with_fullscreen(true);
        }

        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of("video-window"),
            builder,
            |ctx, _class| {
                let pointer_active = ctx.input(|i| i.pointer.is_moving());
                if pointer_active {
                    chrome.set(Instant::now());
                }
                let show_bar = chrome.get().elapsed() < Duration::from_secs(3);

                if show_bar {
                    egui::TopBottomPanel::top("video-window-bar")
                        .frame(
                            egui::Frame::none()
                                .fill(egui::Color32::from_black_alpha(190))
                                .inner_margin(egui::Margin::symmetric(12.0, 7.0)),
                        )
                        .show(ctx, |ui| {
                            ui.horizontal(|ui| {
                                let label = if self.video_fullscreen {
                                    "Выйти из полноэкранного"
                                } else {
                                    "На весь экран"
                                };
                                if t::chip_button(ui, label, false).clicked() {
                                    self.video_fullscreen = !self.video_fullscreen;
                                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(
                                        self.video_fullscreen,
                                    ));
                                }
                                if let Some(remote) = &self.remote {
                                    // миллисекунды по этапам: по ним зритель видит,
                                    // упирается показ в его же компьютер или в сеть
                                    let (decode, gpu, image) = remote.timings();
                                    let buffer = self
                                        .config
                                        .video_buffer_ms
                                        .unwrap_or(crate::network::video_link::DEFAULT_BUFFER_MS)
                                        as f32
                                        / 1000.0;
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "Смотрит: {} · {:.0} FPS · буфер {:.1} с · декод {:.1} / GPU {:.1} / картинка {:.1} мс",
                                            remote.peer,
                                            remote.fps(),
                                            buffer,
                                            decode,
                                            gpu,
                                            image
                                        ))
                                        .size(9.5)
                                        .color(t::TEXT_MUTED),
                                    );
                                } else if let Some(video) = &self.video {
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "{} · {:.0} FPS",
                                            video.source,
                                            video.fps()
                                        ))
                                        .size(9.5)
                                        .color(t::TEXT_MUTED),
                                    );
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if t::icon_button(ui, t::Icon::Close, 24.0, false)
                                        .on_hover_text("Закрыть")
                                        .clicked()
                                    {
                                        close_requested.set(true);
                                    }
                                    ui.label(
                                        egui::RichText::new("Esc — выйти")
                                            .size(9.0)
                                            .color(t::TEXT_MUTED),
                                    );
                                });
                            });
                        });
                }

                egui::CentralPanel::default()
                    .frame(egui::Frame::none().fill(egui::Color32::BLACK))
                    .show(ctx, |ui| {
                        // приоритет у чужого потока
                        let source = self
                            .remote
                            .as_ref()
                            .map(|r| (r.texture(), r.size()))
                            .or_else(|| self.video.as_ref().map(|v| (v.texture(), v.size())));
                        if let Some((Some(tex), (w, h))) = source {
                            let rect = ui.max_rect();
                            let scale =
                                (rect.width() / w as f32).min(rect.height() / h as f32);
                            let size = egui::vec2(w as f32 * scale, h as f32 * scale);
                            let img = egui::Image::new(egui::load::SizedTexture::new(
                                tex.id(),
                                size,
                            ))
                            .sense(egui::Sense::click());
                            let resp = ui.centered_and_justified(|ui| ui.add(img)).inner;
                            if resp.double_clicked() {
                                self.video_fullscreen = !self.video_fullscreen;
                                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(
                                    self.video_fullscreen,
                                ));
                            }
                        } else {
                            // Чёрное окно без объяснения — худший вариант: ошибку
                            // декодера/подписки показываем прямо здесь.
                            let error = self
                                .remote
                                .as_ref()
                                .and_then(|r| r.last_error().cloned())
                                .or_else(|| {
                                    self.video.as_ref().and_then(|v| v.last_error().cloned())
                                })
                                .or_else(|| self.video_error.clone());
                            let text = error
                                .map(|e| format!("Видео недоступно: {e}"))
                                .unwrap_or_else(|| "Ожидание кадра…".to_string());
                            ui.centered_and_justified(|ui| {
                                ui.label(
                                    egui::RichText::new(text)
                                        .size(11.0)
                                        .color(t::TEXT_MUTED),
                                );
                            });
                        }
                    });

                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    if self.video_fullscreen {
                        self.video_fullscreen = false;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                    } else {
                        close_requested.set(true);
                    }
                }
                if ctx.input(|i| i.viewport().close_requested()) {
                    close_requested.set(true);
                }

                if show_bar {
                    ctx.request_repaint_after(Duration::from_millis(200));
                }
            },
        );

        if close_requested.get() {
            self.video_window = false;
            self.video_fullscreen = false;
        }
    }

    /// Сохраняет настройки на диск не чаще раза в 400 мс после изменений
    fn persist_if_needed(&mut self) {
        let mut changed = false;

        let ch1 = self.mixer.ch1_volume();
        if self.config.ch1_volume != Some(ch1) {
            self.config.ch1_volume = Some(ch1);
            changed = true;
        }
        let ch2 = self.mixer.ch2_volume();
        if self.config.ch2_volume != Some(ch2) {
            self.config.ch2_volume = Some(ch2);
            changed = true;
        }
        let out = self.mixer.output_volume();
        if self.config.output_volume != Some(out) {
            self.config.output_volume = Some(out);
            changed = true;
        }
        let in1 = self.mixer.input1_channel();
        if self.config.input1_channel != Some(in1) {
            self.config.input1_channel = Some(in1);
            changed = true;
        }
        let in2 = self.mixer.input2_channel();
        if self.config.input2_channel != Some(in2) {
            self.config.input2_channel = Some(in2);
            changed = true;
        }
        // Профиль — пресет тумблеров; в файл пишем сами тумблеры, а старый
        // ключ профиля — только пока он однозначен (для старых exe).
        let profile1 = self.mixer.input1_profile();
        let legacy1 = match profile1 {
            crate::mixer::SlotProfile::Custom => None,
            p => Some(p.as_u32()),
        };
        if self.config.input1_profile != legacy1 {
            self.config.input1_profile = legacy1;
            changed = true;
        }
        let profile2 = self.mixer.input2_profile();
        let legacy2 = match profile2 {
            crate::mixer::SlotProfile::Custom => None,
            p => Some(p.as_u32()),
        };
        if self.config.input2_profile != legacy2 {
            self.config.input2_profile = legacy2;
            changed = true;
        }
        for (key, value) in [
            ("input1_denoise", self.mixer.input1_denoise()),
            ("input2_denoise", self.mixer.input2_denoise()),
            ("input1_aec", self.mixer.input1_aec()),
            ("input2_aec", self.mixer.input2_aec()),
            ("input1_gate", self.mixer.input1_gate()),
            ("input1_agc", self.mixer.input1_agc()),
            ("input2_gate", self.mixer.input2_gate()),
            ("input2_agc", self.mixer.input2_agc()),
        ] {
            let slot = match key {
                "input1_denoise" => &mut self.config.input1_denoise,
                "input2_denoise" => &mut self.config.input2_denoise,
                "input1_aec" => &mut self.config.input1_aec,
                "input2_aec" => &mut self.config.input2_aec,
                "input1_gate" => &mut self.config.input1_gate,
                "input1_agc" => &mut self.config.input1_agc,
                "input2_gate" => &mut self.config.input2_gate,
                _ => &mut self.config.input2_agc,
            };
            if *slot != Some(value) {
                *slot = Some(value);
                changed = true;
            }
        }
        let bitrate = self.mixer.opus_bitrate();
        if self.config.opus_bitrate != Some(bitrate) {
            self.config.opus_bitrate = Some(bitrate);
            changed = true;
        }
        if self.config.continuous_mode != Some(self.continuous_mode) {
            self.config.continuous_mode = Some(self.continuous_mode);
            changed = true;
        }
        let vk = crate::keybind::CURRENT_VK.load(std::sync::atomic::Ordering::Relaxed);
        if self.config.ptt_vk != Some(vk) {
            self.config.ptt_vk = Some(vk);
            changed = true;
        }
        if self.config.nick.as_deref() != Some(self.nick.as_str()) {
            self.config.nick = Some(self.nick.clone());
            changed = true;
        }
        let tx_limiter = self.mixer.is_tx_limiter_enabled();
        if self.config.tx_limiter != Some(tx_limiter) {
            self.config.tx_limiter = Some(tx_limiter);
            changed = true;
        }
        // Старый общий тумблер — «есть ли хоть один эффект»: так его поймёт
        // exe прежней версии, а наш загрузчик его проигнорирует, если есть
        // новые ключи.
        let legacy_dsp = self.mixer.input1_denoise()
            || self.mixer.input1_gate()
            || self.mixer.input1_agc()
            || self.mixer.input2_denoise()
            || self.mixer.input2_gate()
            || self.mixer.input2_agc();
        if self.config.mic_processing != Some(legacy_dsp) {
            self.config.mic_processing = Some(legacy_dsp);
            changed = true;
        }
        let rx_limiter = self.mixer.is_rx_limiter_enabled();
        if self.config.rx_limiter != Some(rx_limiter) {
            self.config.rx_limiter = Some(rx_limiter);
            changed = true;
        }
        let peer_vols = self.mixer.peer_volumes();
        if self.config.peer_volumes != peer_vols {
            self.config.peer_volumes = peer_vols;
            changed = true;
        }

        if changed {
            self.pending_save = true;
        }

        if self.pending_save && self.last_save.elapsed() >= Duration::from_millis(400) {
            self.config.save();
            self.pending_save = false;
            self.last_save = Instant::now();
        }
    }
}

/// Строка «подпись — значение» в панели диагностики.
fn diag_row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            egui::vec2(132.0, 16.0),
            egui::Label::new(
                egui::RichText::new(label)
                    .size(9.5)
                    .color(t::TEXT_MUTED),
            ),
        );
        ui.label(
            egui::RichText::new(value)
                .size(10.5)
                .monospace()
                .color(t::TEXT),
        );
    });
}

/// Тело панели диагностики. Отдельной функцией: рисует только то, что уже
/// собрано в `ReportInput`, поэтому экран и скопированный отчёт не расходятся.
fn draw_diagnostics_body(ui: &mut egui::Ui, input: &crate::diagnostics::ReportInput) {
    let short = crate::diagnostics::short_id;

    t::section_label(ui, "СЕТЬ");
    ui.add_space(4.0);
    diag_row(ui, "Узел", &short(&input.node_id));
    diag_row(
        ui,
        "Роль",
        if input.is_host { "хост" } else { "гость" },
    );
    diag_row(
        ui,
        "В комнате",
        &format!(
            "{} (вышли: {})",
            members_label(input.members_in_room),
            if input.left_room { "да" } else { "нет" }
        ),
    );
    diag_row(
        ui,
        "Друзья в сети",
        &format!("{} из {}", input.friends_online, input.friends_total),
    );
    if input.links.is_empty() {
        diag_row(ui, "Соединения", "нет");
    } else {
        for link in &input.links {
            let bw = match link.bandwidth_bps {
                Some(bps) => format!(" · {:.1} Мбит/с", bps as f64 / 1_000_000.0),
                None => String::new(),
            };
            diag_row(
                ui,
                &short(&link.node_id),
                &format!(
                    "{} · RTT {} мс · потерь {}{bw}",
                    if link.relay { "релей" } else { "напрямую" },
                    link.rtt_ms,
                    link.lost_packets
                ),
            );
        }
    }

    ui.add_space(10.0);
    t::section_label(ui, "ГОЛОС");
    ui.add_space(4.0);
    diag_row(
        ui,
        "Микрофон",
        input.mic_device.as_deref().unwrap_or("по умолчанию"),
    );
    diag_row(
        ui,
        "Второй вход",
        input.input2_device.as_deref().unwrap_or("то же устройство"),
    );
    diag_row(
        ui,
        "Динамики",
        input.output_device.as_deref().unwrap_or("по умолчанию"),
    );
    diag_row(
        ui,
        "Передача",
        &format!(
            "{} кбит/с · уровень {:.2}{}",
            input.opus_kbps,
            input.mic_level,
            if input.mic_muted { " · микрофон выключен" } else { "" }
        ),
    );
    let all = &input.rx_total;
    diag_row(
        ui,
        "Приём, всего",
        &format!(
            "провалов {} · маскировано {} (FEC {}) · перезапусков {} · выброшено {}",
            all.underruns, all.concealed, all.fec, all.restarts, all.drops
        ),
    );
    let win = &input.rx_window;
    diag_row(
        ui,
        "Приём, окно",
        &format!(
            "за {:.0} с: провалов {} · маскировано {} (FEC {}) · перезапусков {} · выброшено {}",
            input.rx_window_secs, win.underruns, win.concealed, win.fec, win.restarts, win.drops
        ),
    );

    ui.add_space(10.0);
    t::section_label(ui, "ВИДЕО");
    ui.add_space(4.0);
    match &input.own_video {
        Some(v) => diag_row(
            ui,
            "Своя трансляция",
            &format!(
                "показ {:.0}/{:.0} FPS · захват {:.0} · {:.1} Мбит/с · декод {:.1} мс · GPU {:.1} мс · картинка {:.1} мс",
                v.show_fps,
                v.target_fps as f32,
                v.encode_fps as f32,
                v.kbps as f32 / 1000.0,
                v.decode_ms,
                v.gpu_ms,
                v.image_ms
            ),
        ),
        None => diag_row(ui, "Своя трансляция", "не идёт"),
    }
    match &input.remote_video {
        Some(v) => diag_row(
            ui,
            "Приём",
            &format!(
                "{} · {:.0} FPS · кадров {} · буфер {:.1} с",
                short(&v.peer),
                v.fps,
                v.frames,
                v.buffer_ms as f32 / 1000.0
            ),
        ),
        None => diag_row(ui, "Приём", "нет"),
    }

    ui.add_space(10.0);
    t::section_label(ui, "ПРОЧЕЕ");
    ui.add_space(4.0);
    let errors_text = if input.errors.is_empty() {
        "нет".to_string()
    } else {
        input.errors.join(" · ")
    };
    diag_row(ui, "Ошибки", &errors_text);
    diag_row(
        ui,
        "Время работы",
        &format!(
            "{:02}:{:02}:{:02}",
            input.uptime.as_secs() / 3600,
            (input.uptime.as_secs() / 60) % 60,
            input.uptime.as_secs() % 60
        ),
    );
}

/// «0:07» — длительность звонка в оверлее
fn format_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// «4 узла» — русская форма множественного числа
pub fn members_label(n: usize) -> String {
    let word = match n % 10 {
        1 if n % 100 != 11 => "узел",
        2..=4 if !(12..=14).contains(&(n % 100)) => "узла",
        _ => "узлов",
    };
    format!("{} {}", n, word)
}

/// Кнопка чата с непрочитанными
fn chat_chip(ui: &mut egui::Ui, unread: usize, active: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(74.0, 26.0), egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        Rounding::same(7.0),
        if active {
            t::with_alpha(t::BRAND, 0.12)
        } else if response.hovered() {
            t::with_alpha(egui::Color32::WHITE, 0.07)
        } else {
            t::with_alpha(egui::Color32::WHITE, 0.035)
        },
    );
    painter.rect_stroke(rect, Rounding::same(7.0), Stroke::new(1.0_f32, t::STROKE_SOFT));
    t::draw_icon(
        &painter,
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + 18.0, rect.center().y),
            egui::vec2(13.0, 13.0),
        ),
        t::Icon::Chat,
        if active { t::BRAND } else { t::TEXT_DIM },
    );
    painter.text(
        egui::pos2(rect.left() + 29.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "Чат",
        egui::FontId::proportional(10.5),
        if active { t::BRAND } else { t::TEXT_DIM },
    );
    if unread > 0 {
        painter.circle_filled(
            egui::pos2(rect.right() - 12.0, rect.center().y),
            7.0,
            t::BRAND_2,
        );
        painter.text(
            egui::pos2(rect.right() - 12.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            format!("{}", unread.min(9)),
            egui::FontId::proportional(8.0),
            egui::Color32::WHITE,
        );
    }
    response
}

/// Строка микшера: подпись, тонкий слайдер без ручки, значение
fn mixer_row(ui: &mut egui::Ui, label: &str, value: &mut u32, level: f32) {
    let row_h = 24.0;
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), row_h), egui::Sense::hover());
    let painter = ui.painter_at(rect);

    painter.text(
        egui::pos2(rect.left(), rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(8.5),
        t::TEXT_MUTED,
    );
    painter.text(
        egui::pos2(rect.right(), rect.center().y),
        egui::Align2::RIGHT_CENTER,
        format!("{}%", *value),
        egui::FontId::monospace(8.5),
        t::TEXT_DIM,
    );

    let track = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 84.0, rect.center().y - 1.5),
        egui::pos2(rect.right() - 36.0, rect.center().y + 1.5),
    );
    if track.width() <= 0.0 {
        return;
    }
    painter.rect_filled(track, Rounding::same(1.0), t::TRACK);
    let frac = (*value as f32 / 100.0).clamp(0.0, 1.0);
    if frac > 0.0 {
        let fill = egui::Rect::from_min_max(
            track.left_top(),
            egui::pos2(track.left() + track.width() * frac, track.bottom()),
        );
        painter.rect_filled(fill, Rounding::same(1.0), t::TRACK_FILL);
    }
    if level > 0.02 {
        let vu = egui::Rect::from_min_size(
            egui::pos2(track.left(), track.bottom() + 2.0),
            egui::vec2(track.width() * level.clamp(0.0, 1.0), 1.0),
        );
        painter.rect_filled(vu, 0.0, t::with_alpha(t::LIVE, 0.35));
    }

    let resp = ui
        .interact(
            track.expand2(egui::vec2(0.0, 8.0)),
            ui.id().with(label),
            egui::Sense::click_and_drag(),
        )
        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    if (resp.dragged() || resp.clicked()) && resp.interact_pointer_pos().is_some() {
        let x = resp.interact_pointer_pos().unwrap().x;
        let f = ((x - track.left()) / track.width()).clamp(0.0, 1.0);
        *value = (f * 100.0).round() as u32;
    }
}









