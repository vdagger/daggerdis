//! Графический интерфейс: компактная эфирная консоль с оверлеями

pub mod theme;

mod stream_control;
mod chat_overlay;
mod call_overlay;
mod diagnostics_panel;
mod settings_overlay;
mod video_panel;
mod console;
mod persist;
mod devices_ui;

#[cfg(windows)]
mod video_session;

use eframe::egui;
use egui::{Align, FontFamily, FontId, Layout, Rounding, Stroke, TextStyle};
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

    fn show_toast(&mut self, text: &str) {
        self.toast = Some((text.to_string(), Instant::now()));
    }

    fn copy_node_id(&mut self, ctx: &egui::Context) {
        ctx.output_mut(|o| o.copied_text = self.my_node_id.clone());
        self.show_toast("NodeID скопирован — отправьте другу");
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
            crate::calls::CallView::Incoming { nick, peer, .. } => {
                Some(self.peer_display_name(&peer, &nick))
            }
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
        // Длинные строки (счётчики приёма) переносим, а не обрезаем краем
        // панели: обрезанное «потеряно 0 из …» читается как ошибка.
        ui.add(
            egui::Label::new(
                egui::RichText::new(value)
                    .size(10.5)
                    .monospace()
                    .color(t::TEXT),
            )
            .wrap(),
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
        &crate::diagnostics::rx_line(all),
    );
    let win = &input.rx_window;
    diag_row(
        ui,
        "Приём, окно",
        &format!(
            "за {:.0} с: {}",
            input.rx_window_secs,
            crate::diagnostics::rx_line(win)
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

