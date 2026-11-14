//! Графический интерфейс: тёмная тема с акцентом на эфир

pub mod theme;

mod stream_control;

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
    my_node_id: String,
    network_error: Option<String>,
    nick: String,
    add_friend_node: String,
    reset_note: Option<String>,
    pending_save: bool,
    last_save: Instant,
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
            my_node_id,
            network_error,
            nick,
            add_friend_node: String::new(),
            reset_note: None,
            pending_save: false,
            last_save: Instant::now(),
        }
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

        self.persist_if_needed();

        ctx.request_repaint();
    }
}

impl RadioApp {
    fn draw_main(&mut self, ui: &mut egui::Ui, level: f32, transmitting: bool, wave: &[f32]) {
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
                        );
                    });

                ui.add_space(10.0);
            });
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
