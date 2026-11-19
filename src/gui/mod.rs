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
    prev_input_index: std::sync::atomic::AtomicU32,
    /// Выбор второго входа: 0 — «то же устройство», n — устройство n-1.
    selected_input2: Arc<std::sync::atomic::AtomicU32>,
    prev_input2: std::sync::atomic::AtomicU32,
    /// Почему второй вход молчит (занят, выдернут) — строкой в настройках.
    input2_error: Option<String>,
    input_devices: Vec<String>,
    input_channel_counts: Vec<u32>,
    output_devices: Vec<String>,
    is_host: Arc<AtomicBool>,
    playback_stream: Arc<Mutex<Option<cpal::Stream>>>,
    playback_rx: Arc<std::sync::Mutex<tokio::sync::mpsc::Receiver<(u64, u32, Vec<u8>)>>>,
    prev_output_index: std::sync::atomic::AtomicU32,
    continuous_mode: bool,
    theme_configured: bool,
    settings_open: bool,
    chat_open: bool,
    toast: Option<(String, Instant)>,
    wave: VecDeque<f32>,
    config: crate::config::AppConfig,
    friends: Arc<crate::friends::FriendsManager>,
    chat_log: Arc<crate::chat::ChatLog>,
    calls: Arc<crate::calls::CallManager>,
    /// Кто из друзей недавно подавал признаки жизни (ping/pong).
    presence: Arc<crate::presence::PresenceTracker>,
    active_chat: Option<String>,
    /// Собеседник, для которого сейчас набран `chat_input`. Смена диалога
    /// чистит черновик, а не переносит его в чужую переписку.
    chat_target: Option<String>,
    chat_input: String,
    /// Счётчики непрочитанного на прошлом кадре: рост = новое сообщение,
    /// о нём сообщаем тостом, потому что бейдж легко пропустить.
    seen_unread: std::collections::HashMap<String, usize>,
    /// Окно уже мигнуло в панели задач за текущий входящий звонок.
    call_attention: bool,
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
    #[cfg(windows)]
    video_link: Option<Arc<crate::network::video_link::VideoLink>>,
    #[cfg(windows)]
    video_rt: Option<tokio::runtime::Handle>,
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
        my_node_id: String,
        network_error: Option<String>,
        calls: Arc<crate::calls::CallManager>,
        presence: Arc<crate::presence::PresenceTracker>,
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
            prev_input_index: std::sync::atomic::AtomicU32::new(initial_input),
            selected_input2: std::sync::Arc::new(std::sync::atomic::AtomicU32::new(initial_input2)),
            prev_input2: std::sync::atomic::AtomicU32::new(initial_input2),
            input2_error: None,
            input_devices,
            input_channel_counts,
            output_devices,
            is_host,
            playback_stream,
            playback_rx,
            prev_output_index: std::sync::atomic::AtomicU32::new(initial_output),
            continuous_mode: config.continuous_mode.unwrap_or(false),
            theme_configured: false,
            settings_open: false,
            chat_open: false,
            toast: None,
            wave: VecDeque::new(),
            config,
            friends,
            chat_log,
            calls,
            presence,
            active_chat: None,
            chat_target: None,
            chat_input: String::new(),
            seen_unread: std::collections::HashMap::new(),
            call_attention: false,
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
            video_link: None,
            #[cfg(windows)]
            video_rt: None,
        }
    }

    /// Подключает видео к сети: транспорт и рантайм для исходящих соединений.
    #[cfg(windows)]
    pub fn attach_video(
        &mut self,
        link: Arc<crate::network::video_link::VideoLink>,
        rt: tokio::runtime::Handle,
    ) {
        self.video_link = Some(link);
        self.video_rt = Some(rt);
    }

    /// Подменяет входные потоки, когда в настройках выбрали другие устройства.
    /// Второй поток существует только при отдельном устройстве; «То же
    /// устройство» снова собирает один поток с двух каналов первой карты.
    fn reconcile_inputs(&mut self) {
        let current_input = self
            .selected_input
            .load(std::sync::atomic::Ordering::Relaxed);
        let current_input2 = self
            .selected_input2
            .load(std::sync::atomic::Ordering::Relaxed);
        let prev_input = self
            .prev_input_index
            .load(std::sync::atomic::Ordering::Relaxed);
        let prev_input2 = self.prev_input2.load(std::sync::atomic::Ordering::Relaxed);
        if current_input == prev_input && current_input2 == prev_input2 {
            return;
        }
        self.prev_input_index
            .store(current_input, std::sync::atomic::Ordering::Relaxed);
        self.prev_input2
            .store(current_input2, std::sync::atomic::Ordering::Relaxed);

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
    }

    fn show_toast(&mut self, text: &str) {
        self.toast = Some((text.to_string(), Instant::now()));
    }

    fn copy_node_id(&mut self, ctx: &egui::Context) {
        ctx.output_mut(|o| o.copied_text = self.my_node_id.clone());
        self.show_toast("NodeID скопирован — отправьте другу");
    }

    fn unread_total(&self) -> usize {
        self.friends
            .friends()
            .iter()
            .map(|f| self.chat_log.unread(&f.node_id))
            .sum()
    }

    fn open_chat(&mut self) {
        if self.active_chat.is_none() {
            self.active_chat = self.friends.friends().first().map(|f| f.node_id.clone());
        }
        self.chat_open = self.active_chat.is_some();
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

        // Входящий звонок: подсветить приложение в панели задач, если оно
        // свёрнуто или закрыто другим окном, — иначе звонок пропускают.
        if matches!(self.calls.view(), crate::calls::CallView::Incoming { .. }) {
            if !self.call_attention {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(
                    egui::UserAttentionType::Critical,
                ));
                self.call_attention = true;
            }
        } else {
            self.call_attention = false;
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

            // чужие кадры из сети; за один апдейт не больше нескольких,
            // чтобы очередь декодирования не растягивала кадр GUI
            if let Some(link) = &self.video_link {
                let mut budget = 4;
                while budget > 0 {
                    let Some(frame) = link.try_recv_remote() else {
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
                }
            }
            // забрать готовый кадр, даже если новых не пришло
            if let Some(remote) = &mut self.remote {
                remote.tick(ctx);
            }
        }

        self.reconcile_inputs();

        let current_output_index = self
            .selected_output
            .load(std::sync::atomic::Ordering::Relaxed);
        let prev_output_index = self
            .prev_output_index
            .load(std::sync::atomic::Ordering::Relaxed);
        if current_output_index != prev_output_index {
            if let Some(device_name) = self.output_devices.get(current_output_index as usize).cloned() {
                stream_control::restart_playback_stream(
                    &self.playback_stream,
                    self.playback_rx.clone(),
                    device_name.clone(),
                    self.mixer.clone(),
                );
                self.config.output_device = Some(device_name);
                self.pending_save = true;
            }
            self.prev_output_index
                .store(current_output_index, std::sync::atomic::Ordering::Relaxed);
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
        if self.settings_open && self.chat_open {
            self.chat_open = false;
        }
        if self.settings_open {
            self.draw_settings_overlay(ctx);
        }
        if self.chat_open {
            self.draw_chat_overlay(ctx);
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
                                        link.stop_viewers();
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
                                            if let (Some(link), Some(rt)) =
                                                (self.video_link.clone(), self.video_rt.clone())
                                            {
                                                for peer in self.room.get_all_peers() {
                                                    if peer.address != self.my_node_id {
                                                        let link = link.clone();
                                                        let target = peer.address.clone();
                                                        rt.spawn(async move {
                                                            link.add_viewer(target).await;
                                                        });
                                                    }
                                                }
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
        let Some(node_id) = self.active_chat.clone() else {
            self.chat_open = false;
            return;
        };
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
                        crate::widgets::chat_panel::draw_chat_panel(
                            ui,
                            &self.chat_log,
                            &self.friends,
                            &node_id,
                            &mut self.chat_input,
                            self.iroh.clone(),
                            self.my_id,
                            &mut close,
                        );
                    });
            });

        // Открытый чат считается прочитанным: раньше счётчик копился вечно,
        // и бейдж горел даже при уже просмотренной переписке.
        self.chat_log.mark_read(&node_id);
        self.seen_unread.insert(node_id.clone(), 0);

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
        if !ctx.wants_keyboard_input() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            match view {
                crate::calls::CallView::Incoming { .. } => decline = true,
                crate::calls::CallView::Outgoing { .. } => cancel = true,
                crate::calls::CallView::Active { .. } => end = true,
                crate::calls::CallView::Idle => {}
            }
        }

        if let crate::calls::CallView::Active { nick, elapsed } = &view {
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
                                    egui::RichText::new(nick)
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
                                            egui::RichText::new("Завершить")
                                                .size(10.0)
                                                .strong()
                                                .color(egui::Color32::WHITE),
                                        )
                                        .fill(t::REC)
                                        .rounding(Rounding::same(7.0)),
                                    )
                                    .on_hover_text("Завершить звонок (ESC)")
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
                                            egui::RichText::new("Входящий звонок")
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
                                    if t::primary_button(ui, "Принять", egui::vec2(150.0, 30.0))
                                        .clicked()
                                    {
                                        accept = true;
                                    }
                                });
                                ui.add_space(4.0);
                                t::micro(ui, "ESC — отклонить");
                            }
                            crate::calls::CallView::Outgoing { nick, elapsed } => {
                                ui.horizontal(|ui| {
                                    t::avatar(ui, nick, 34.0, false);
                                    ui.add_space(4.0);
                                    ui.vertical(|ui| {
                                        ui.label(
                                            egui::RichText::new("Звоним…")
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
                                            egui::RichText::new("Отменить звонок")
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
                                t::micro(ui, "ESC — отменить");
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
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "Смотрит: {} · {:.0} FPS · декод {:.1} / GPU {:.1} / картинка {:.1} мс",
                                            remote.peer,
                                            remote.fps(),
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
            ("input1_gate", self.mixer.input1_gate()),
            ("input1_agc", self.mixer.input1_agc()),
            ("input2_gate", self.mixer.input2_gate()),
            ("input2_agc", self.mixer.input2_agc()),
        ] {
            let slot = match key {
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
        let legacy_dsp = self.mixer.input1_gate()
            || self.mixer.input1_agc()
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

