//! Графический интерфейс: тёмная тема с акцентом на эфир

pub mod theme;

mod stream_control;

#[cfg(windows)]
mod video_session;

use eframe::egui;
use egui::{FontFamily, FontId, Rounding, Stroke, TextStyle};
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
    prev_input_index: std::sync::atomic::AtomicU32,
    packet_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
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
    wave: VecDeque<f32>,
    config: crate::config::AppConfig,
    friends: Arc<crate::friends::FriendsManager>,
    chat_log: Arc<crate::chat::ChatLog>,
    active_chat: Option<String>,
    chat_input: String,
    my_node_id: String,
    network_error: Option<String>,
    nick: String,
    add_friend_node: String,
    reset_note: Option<String>,
    pending_save: bool,
    last_save: Instant,
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
        packet_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
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
    ) -> Self {
        let initial_input = selected_input.load(std::sync::atomic::Ordering::Relaxed);
        let initial_output = selected_output.load(std::sync::atomic::Ordering::Relaxed);
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
            prev_input_index: std::sync::atomic::AtomicU32::new(initial_input),
            packet_tx,
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
            wave: VecDeque::new(),
            config,
            friends,
            chat_log,
            active_chat: None,
            chat_input: String::new(),
            my_node_id,
            network_error,
            nick,
            add_friend_node: String::new(),
            reset_note: None,
            pending_save: false,
            last_save: Instant::now(),
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

    fn apply_theme(&mut self, ctx: &egui::Context) {
        if self.theme_configured {
            return;
        }

        let mut visuals = egui::Visuals::dark();
        visuals.panel_fill = t::BG;
        visuals.window_fill = t::SURFACE;
        visuals.extreme_bg_color = egui::Color32::from_rgb(12, 14, 18);
        visuals.faint_bg_color = t::SURFACE_HI;
        visuals.window_stroke = Stroke::new(1.0_f32, t::STROKE);
        visuals.window_rounding = Rounding::same(14.0);
        visuals.menu_rounding = Rounding::same(10.0);
        visuals.window_shadow = egui::epaint::Shadow {
            offset: egui::vec2(0.0, 10.0),
            blur: 28.0,
            spread: 0.0,
            color: egui::Color32::from_black_alpha(140),
        };
        visuals.popup_shadow = visuals.window_shadow;

        visuals.widgets.inactive.bg_fill = t::SURFACE_HI;
        visuals.widgets.inactive.weak_bg_fill = t::SURFACE;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, t::STROKE);
        visuals.widgets.inactive.rounding = Rounding::same(9.0);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, t::TEXT_DIM);

        visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(36, 40, 48);
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, t::with_alpha(t::BRAND, 0.45));
        visuals.widgets.hovered.rounding = Rounding::same(9.0);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, t::TEXT);

        visuals.widgets.active.bg_fill = t::with_alpha(t::BRAND, 0.22);
        visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, t::BRAND);
        visuals.widgets.active.rounding = Rounding::same(9.0);
        visuals.widgets.active.fg_stroke = Stroke::new(1.0_f32, t::TEXT);

        visuals.selection.bg_fill = t::with_alpha(t::BRAND, 0.35);
        visuals.selection.stroke = Stroke::new(1.0_f32, t::TEXT);
        visuals.override_text_color = Some(t::TEXT);
        visuals.hyperlink_color = t::BRAND;

        ctx.set_visuals(visuals);

        let mut style = (*ctx.style()).clone();
        style.text_styles = [
            (TextStyle::Heading, FontId::new(18.0, FontFamily::Proportional)),
            (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
            (TextStyle::Button, FontId::new(12.5, FontFamily::Proportional)),
            (TextStyle::Small, FontId::new(10.5, FontFamily::Proportional)),
            (TextStyle::Monospace, FontId::new(11.5, FontFamily::Monospace)),
        ]
        .into();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.slider_width = 140.0;
        style.spacing.window_margin = egui::Margin::same(14.0);
        ctx.set_style(style);

        self.theme_configured = true;
    }
}

impl eframe::App for RadioApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_theme(ctx);

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

        let current_input_index = self
            .selected_input
            .load(std::sync::atomic::Ordering::Relaxed);
        let prev_input_index = self
            .prev_input_index
            .load(std::sync::atomic::Ordering::Relaxed);
        if current_input_index != prev_input_index {
            if let Some(device_name) = self.input_devices.get(current_input_index as usize).cloned() {
                stream_control::restart_audio_stream(
                    &self.mic_stream,
                    crate::audio::UniversalAudioCapture::new().unwrap(),
                    self.packet_tx.clone(),
                    self.ptt_flag.clone(),
                    self.mixer.clone(),
                    device_name.clone(),
                );
                self.config.input_device = Some(device_name);
                self.pending_save = true;
            }
            self.prev_input_index
                .store(current_input_index, std::sync::atomic::Ordering::Relaxed);
        }

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
            .frame(
                egui::Frame::none()
                    .fill(t::BG)
                    .inner_margin(egui::Margin::symmetric(16.0, 10.0)),
            )
            .show(ctx, |ui| {
                crate::widgets::info_panel::draw_top_bar(
                    ui,
                    self.my_id,
                    &mut self.settings_open,
                );
            });

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(t::BG)
                    .inner_margin(egui::Margin::symmetric(16.0, 6.0)),
            )
            .show(ctx, |ui| {
                if self.settings_open {
                    self.draw_settings(ui);
                } else {
                    self.draw_main(ui, level, transmitting, &wave);
                }
            });

        #[cfg(windows)]
        self.draw_video_window(ctx);

        self.persist_if_needed();

        ctx.request_repaint();
    }
}

impl RadioApp {
    fn draw_main(&mut self, ui: &mut egui::Ui, level: f32, transmitting: bool, wave: &[f32]) {
        if let Some(active) = self.active_chat.clone() {
            self.chat_log.mark_read(&active);
        }

        // Мягкий градиент и брендовое свечение вместо плоской заливки
        let full = ui.max_rect();
        let mut mesh = egui::Mesh::default();
        let top = egui::Color32::from_rgb(17, 19, 25);
        mesh.colored_vertex(full.left_top(), top);
        mesh.colored_vertex(full.right_top(), top);
        mesh.colored_vertex(full.right_bottom(), t::BG);
        mesh.colored_vertex(full.left_bottom(), t::BG);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        ui.painter().add(egui::Shape::mesh(mesh));

        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let peer_count = self.room.get_all_peers().len();
                let is_host = self.is_host.load(std::sync::atomic::Ordering::Relaxed);

                if let Some(err) = &self.network_error {
                    egui::Frame::none()
                        .fill(t::with_alpha(t::REC, 0.12))
                        .rounding(Rounding::same(10.0))
                        .stroke(Stroke::new(1.0_f32, t::with_alpha(t::REC, 0.4)))
                        .inner_margin(egui::Margin::same(10.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(egui::RichText::new(err).size(11.0).color(t::REC));
                        });
                    ui.add_space(10.0);
                }

                crate::widgets::hero::draw_hero(ui, level, transmitting, is_host, wave, peer_count);

                ui.add_space(12.0);

                #[cfg(windows)]
                {
                    self.draw_video_panel(ui);
                    ui.add_space(12.0);
                }

                crate::widgets::ptt_button::draw_ptt_button(
                    ui,
                    &self.ptt_flag,
                    &mut self.continuous_mode,
                    &self.mixer,
                );

                ui.add_space(16.0);

                egui::Frame::none()
                    .fill(t::SURFACE)
                    .rounding(Rounding::same(12.0))
                    .stroke(Stroke::new(1.0_f32, t::STROKE))
                    .inner_margin(egui::Margin::same(12.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        crate::widgets::identity::draw_my_node_id(ui, &self.my_node_id);
                    });

                ui.add_space(10.0);

                crate::widgets::room_panel::draw_participants(
                    ui,
                    self.room.clone(),
                    &self.mixer,
                    self.my_id,
                    self.is_host.clone(),
                    transmitting,
                );

                ui.add_space(10.0);

                egui::Frame::none()
                    .fill(t::SURFACE)
                    .rounding(Rounding::same(12.0))
                    .stroke(Stroke::new(1.0_f32, t::STROKE))
                    .inner_margin(egui::Margin::same(12.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        crate::widgets::room_panel::draw_room_panel(
                            ui,
                            &mut self.connect_ip_input,
                            self.iroh.clone(),
                            self.my_id,
                            self.is_host.clone(),
                            self.room.clone(),
                        );
                    });

                ui.add_space(10.0);

                if let Some(active) = self.active_chat.clone() {
                    ui.add_space(10.0);
                    let mut close = false;
                    egui::Frame::none()
                        .fill(t::SURFACE)
                        .rounding(Rounding::same(12.0))
                        .stroke(Stroke::new(1.0_f32, t::STROKE))
                        .inner_margin(egui::Margin::same(12.0))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            crate::widgets::chat_panel::draw_chat_panel(
                                ui,
                                &self.chat_log,
                                &self.friends,
                                &active,
                                &mut self.chat_input,
                                self.iroh.clone(),
                                self.my_id,
                                &mut close,
                            );
                        });
                    if close {
                        self.active_chat = None;
                    }
                }

                egui::Frame::none()
                    .fill(t::SURFACE)
                    .rounding(Rounding::same(12.0))
                    .stroke(Stroke::new(1.0_f32, t::STROKE))
                    .inner_margin(egui::Margin::same(12.0))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        crate::widgets::friends_panel::draw_friends_panel(
                            ui,
                            &self.friends,
                            self.iroh.clone(),
                            self.my_id,
                            &self.my_node_id,
                            &self.nick,
                            self.is_host.clone(),
                            &mut self.add_friend_node,
                            &self.chat_log,
                            &mut self.active_chat,
                        );
                    });

                ui.add_space(10.0);
            });
    }

    /// Трансляция экрана: запуск/остановка и локальное превью.
    #[cfg(windows)]
    fn draw_video_panel(&mut self, ui: &mut egui::Ui) {
        let mut open_window = false;
        egui::Frame::none()
            .fill(t::SURFACE)
            .rounding(Rounding::same(12.0))
            .stroke(Stroke::new(1.0_f32, t::STROKE))
            .inner_margin(egui::Margin::same(12.0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let running = self.video.is_some();
                // 30 или 60 fps; меняется только до старта трансляции
                let fps = match self.config.video_fps {
                    Some(30) => 30,
                    _ => 60,
                };

                ui.horizontal(|ui| {
                    let label = if running {
                        "Остановить трансляцию"
                    } else {
                        "Транслировать экран"
                    };
                    if ui
                        .add(
                            egui::Button::new(egui::RichText::new(label).size(11.0).strong())
                                .rounding(Rounding::same(8.0)),
                        )
                        .clicked()
                    {
                        if running {
                            self.video = None;
                            self.video_fullscreen = false;
                        } else {
                            match video_session::VideoSession::start(fps) {
                                Ok(v) => {
                                    self.video = Some(v);
                                    self.video_error = None;
                                    // раздаём поток всем, кто уже в комнате
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

                    if running {
                        if let Some(v) = &self.video {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} · показ {:.0} · захват {} FPS",
                                    v.source,
                                    v.fps(),
                                    v.encode_fps()
                                ))
                                .size(10.0)
                                .color(t::TEXT_MUTED),
                            );
                        }
                    } else {
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("Кадров/с").size(10.0).color(t::TEXT_MUTED),
                        );
                        let mut new_fps = fps;
                        egui::ComboBox::from_id_source("video_fps_combo")
                            .width(64.0)
                            .selected_text(format!("{fps}"))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut new_fps, 30, "30");
                                ui.selectable_value(&mut new_fps, 60, "60");
                            });
                        if new_fps != fps {
                            self.config.video_fps = Some(new_fps);
                            self.pending_save = true;
                        }
                    }
                });

                if let Some(err) = &self.video_error {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(err).size(10.5).color(t::REC));
                }

                if let Some(v) = &self.video {
                    ui.add_space(6.0);
                    if let Some(err) = v.last_error() {
                        ui.label(egui::RichText::new(err).size(10.0).color(t::REC));
                    }
                    match v.texture() {
                        Some(tex) => {
                            let (w, h) = v.size();
                            let avail = ui.available_width();
                            let scale = (avail / w as f32).min(1.0);
                            let size = egui::vec2(w as f32 * scale, h as f32 * scale);
                            let img = egui::Image::new(egui::load::SizedTexture::new(
                                tex.id(),
                                size,
                            ))
                            .rounding(Rounding::same(8.0))
                            .sense(egui::Sense::click());
                            let resp = ui.add(img);
                            if resp.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            if resp.clicked() {
                                open_window = true;
                            }
                        }
                        None => {
                            ui.label(
                                egui::RichText::new("Жду первый кадр…")
                                    .size(10.5)
                                    .color(t::TEXT_MUTED),
                            );
                        }
                    }
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new("Клик по кадру открывает окно просмотра")
                            .size(9.5)
                            .color(t::TEXT_MUTED),
                    );
                }

                // чужой поток: смотрим, что транслирует собеседник
                if let Some(remote) = &self.remote {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "Смотрит: {} · {:.0} FPS",
                            remote.peer,
                            remote.fps()
                        ))
                        .size(10.0)
                        .color(t::TEXT_MUTED),
                    );
                    if let Some(err) = remote.last_error() {
                        ui.label(egui::RichText::new(err).size(10.0).color(t::REC));
                    }
                    if let Some(tex) = remote.texture() {
                        let (w, h) = remote.size();
                        let avail = ui.available_width();
                        let scale = (avail / w as f32).min(1.0);
                        let size = egui::vec2(w as f32 * scale, h as f32 * scale);
                        let img = egui::Image::new(egui::load::SizedTexture::new(
                            tex.id(),
                            size,
                        ))
                        .rounding(Rounding::same(8.0))
                        .sense(egui::Sense::click());
                        if ui.add(img).clicked() {
                            open_window = true;
                        }
                    }
                }
            });
        if open_window {
            self.video_window = true;
        }
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
                egui::TopBottomPanel::top("video-window-bar")
                    .frame(
                        egui::Frame::none()
                            .fill(egui::Color32::from_rgb(16, 18, 22))
                            .inner_margin(egui::Margin::symmetric(8.0, 6.0)),
                    )
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            let label = if self.video_fullscreen {
                                "Выйти из полноэкранного"
                            } else {
                                "На весь экран"
                            };
                            if ui.button(label).clicked() {
                                self.video_fullscreen = !self.video_fullscreen;
                            }
                            if let Some(remote) = &self.remote {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "Смотрит: {} · {:.0} FPS",
                                        remote.peer,
                                        remote.fps()
                                    ))
                                    .size(10.0)
                                    .color(t::TEXT_MUTED),
                                );
                            } else if let Some(video) = &self.video {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} · {:.0} FPS",
                                        video.source,
                                        video.fps()
                                    ))
                                    .size(10.0)
                                    .color(t::TEXT_MUTED),
                                );
                            }
                            if ui.button("Закрыть").clicked() {
                                close_requested.set(true);
                            }
                        });
                    });

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
                            }
                        }
                    });

                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    if self.video_fullscreen {
                        self.video_fullscreen = false;
                    } else {
                        close_requested.set(true);
                    }
                }
                if ctx.input(|i| i.viewport().close_requested()) {
                    close_requested.set(true);
                }
            },
        );

        if close_requested.get() {
            self.video_window = false;
            self.video_fullscreen = false;
        }
    }

    fn draw_settings(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if back_button(ui).clicked() {
                self.settings_open = false;
            }
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new("Настройки")
                    .size(15.0)
                    .strong()
                    .color(t::TEXT),
            );
        });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(10.0);

        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                crate::widgets::audio_settings::draw_audio_settings(
                    ui,
                    &self.input_devices,
                    &self.input_channel_counts,
                    &self.output_devices,
                    &self.selected_input,
                    &self.selected_output,
                    &self.mixer,
                    &mut self.ptt_bind,
                );

                ui.add_space(16.0);
                ui.separator();
                ui.add_space(12.0);

                t::section_label(ui, "ЛИЧНОСТЬ");
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Ваше имя")
                            .size(10.5)
                            .color(t::TEXT_MUTED),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut self.nick).desired_width(160.0),
                    );
                });

                ui.add_space(6.0);
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("Сбросить личность")
                                .size(11.0)
                                .strong()
                                .color(t::REC),
                        )
                        .rounding(Rounding::same(8.0)),
                    )
                    .clicked()
                {
                    crate::identity::Identity::reset();
                    self.reset_note =
                        Some("Новый NodeID создан. Перезапустите приложение.".to_string());
                }
                if let Some(note) = &self.reset_note {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(note).size(9.5).color(t::TEXT_MUTED),
                    );
                }
            });
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

/// Стрелка «назад», нарисованная линиями (в шрифте нет надёжного глифа)
fn back_button(ui: &mut egui::Ui) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(24.0, 24.0), egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let painter = ui.painter_at(rect);
    let color = if response.hovered() {
        t::TEXT
    } else {
        t::TEXT_DIM
    };
    let center = rect.center();
    let stroke = Stroke::new(1.6_f32, color);
    painter.line_segment(
        [egui::pos2(center.x + 3.0, center.y - 6.0), egui::pos2(center.x - 3.0, center.y)],
        stroke,
    );
    painter.line_segment(
        [egui::pos2(center.x - 3.0, center.y), egui::pos2(center.x + 3.0, center.y + 6.0)],
        stroke,
    );
    response
}
