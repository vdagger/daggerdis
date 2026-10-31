//! Графический интерфейс в монолитном стиле Cyber Slate

mod stream_control;

use eframe::egui;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::sync::Mutex;

pub struct RadioApp {
    mixer: Arc<crate::mixer::AudioMixer>,
    ptt_bind: Option<crate::keybind::PttBind>,
    ptt_flag: Arc<AtomicBool>,
    room: Arc<crate::room::RoomManager>,
    room_id_input: String,
    connect_ip_input: String,
    connect_port_input: String,
    my_external_ip: String,
    selected_input: Arc<std::sync::atomic::AtomicU32>,
    selected_output: Arc<std::sync::atomic::AtomicU32>,
    network: Arc<crate::network::NetworkManager>,
    my_id: u64,
    ext_ip: Arc<Mutex<String>>,
    mic_stream: Arc<Mutex<Option<cpal::Stream>>>,
    prev_input_index: std::sync::atomic::AtomicU32,
    packet_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    input_devices: Vec<String>,
    output_devices: Vec<String>,
    is_host: Arc<AtomicBool>,
    playback_stream: Arc<Mutex<Option<cpal::Stream>>>,
    playback_rx: Arc<std::sync::Mutex<tokio::sync::mpsc::Receiver<(u64, Vec<u8>)>>>,
    prev_output_index: std::sync::atomic::AtomicU32,
    continuous_mode: bool,
    theme_configured: bool,
}

impl RadioApp {
    pub fn new(
        mixer: Arc<crate::mixer::AudioMixer>,
        room: Arc<crate::room::RoomManager>,
        ptt_bind: Option<crate::keybind::PttBind>,
        ptt_flag: Arc<AtomicBool>,
        selected_input: Arc<std::sync::atomic::AtomicU32>,
        selected_output: Arc<std::sync::atomic::AtomicU32>,
        network: Arc<crate::network::NetworkManager>,
        my_id: u64,
        ext_ip: Arc<Mutex<String>>,
        mic_stream: Arc<Mutex<Option<cpal::Stream>>>,
        packet_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
        input_devices: Vec<String>,
        output_devices: Vec<String>,
        is_host: Arc<AtomicBool>,
        playback_stream: Arc<Mutex<Option<cpal::Stream>>>,
        playback_rx: Arc<std::sync::Mutex<tokio::sync::mpsc::Receiver<(u64, Vec<u8>)>>>, 
    ) -> Self {
        Self {
            mixer,
            ptt_bind,
            ptt_flag,
            room,
            room_id_input: String::new(),
            connect_ip_input: String::new(),
            connect_port_input: "8080".to_string(),
            my_external_ip: String::new(),
            selected_input,
            selected_output,
            network,
            my_id,
            ext_ip,
            mic_stream,
            prev_input_index: std::sync::atomic::AtomicU32::new(0),
            packet_tx,
            input_devices,
            output_devices,
            is_host,
            playback_stream,
            playback_rx,
            prev_output_index: std::sync::atomic::AtomicU32::new(0),
            continuous_mode: false,
            theme_configured: false,
        }
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        if self.theme_configured {
            return;
        }

        let mut visuals = egui::Visuals::dark();
        
        // Глубокий, контрастный темный фон
        visuals.panel_fill = egui::Color32::from_rgb(26, 28, 32);       // #1A1C20
        visuals.window_fill = egui::Color32::from_rgb(38, 41, 47);      // #26292F
        visuals.extreme_bg_color = egui::Color32::from_rgb(18, 19, 22); // #121316
        
        // Кнопки и акценты: стильный Electric Cyan / Deep Sky Blue (в цвет NodeID)
        visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(50, 54, 62);
        visuals.widgets.inactive.rounding = egui::Rounding::same(6.0);
        visuals.widgets.inactive.fg_stroke.color = egui::Color32::WHITE;
        
        visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(14, 165, 233); // #0EA5E9
        visuals.widgets.hovered.rounding = egui::Rounding::same(6.0);
        visuals.widgets.hovered.fg_stroke.color = egui::Color32::WHITE;
        
        visuals.widgets.active.bg_fill = egui::Color32::from_rgb(2, 132, 199);  // #0284C7
        visuals.widgets.active.rounding = egui::Rounding::same(6.0);
        visuals.widgets.active.fg_stroke.color = egui::Color32::WHITE;

        // Полосы прогресса и выделение в цвет NodeID
        visuals.selection.bg_fill = egui::Color32::from_rgb(14, 165, 233);
        visuals.selection.stroke.color = egui::Color32::WHITE;

        ctx.set_visuals(visuals);

        let mut style = (*ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(8.0, 8.0);
        style.spacing.window_margin = egui::Margin::same(12.0);
        ctx.set_style(style);

        self.theme_configured = true;
    }
}

impl eframe::App for RadioApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_theme(ctx);

        if let Ok(ip_lock) = self.ext_ip.try_lock() {
            self.my_external_ip = ip_lock.clone();
        }

        let current_input_index = self.selected_input.load(std::sync::atomic::Ordering::Relaxed);
        let prev_input_index = self.prev_input_index.load(std::sync::atomic::Ordering::Relaxed);
        if current_input_index != prev_input_index {
            let device_name = self.input_devices[current_input_index as usize].clone();
            stream_control::restart_audio_stream(
                &self.mic_stream,
                crate::audio::UniversalAudioCapture::new().unwrap(),
                self.packet_tx.clone(),
                self.ptt_flag.clone(),
                self.mixer.clone(),
                device_name,
            );
            self.prev_input_index.store(current_input_index, std::sync::atomic::Ordering::Relaxed);
        }

        let current_output_index = self.selected_output.load(std::sync::atomic::Ordering::Relaxed);
        let prev_output_index = self.prev_output_index.load(std::sync::atomic::Ordering::Relaxed);
        if current_output_index != prev_output_index {
            if let Some(device_name) = self.output_devices.get(current_output_index as usize) {
                stream_control::restart_playback_stream(
                    &self.playback_stream,
                    self.playback_rx.clone(),
                    device_name.clone(),
                    self.mixer.clone(),
                );
            }
            self.prev_output_index.store(current_output_index, std::sync::atomic::Ordering::Relaxed);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                let full_width = ui.available_width();
                let card_stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(52, 56, 64));

                // 1. Шапка
                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(38, 41, 47))
                    .stroke(card_stroke)
                    .rounding(egui::Rounding::same(8.0))
                    .inner_margin(egui::Margin::same(12.0))
                    .show(ui, |ui| {
                        ui.set_width(full_width);
                        crate::widgets::info_panel::draw_info_panel(ui, self.my_id, &self.my_external_ip);
                    });
                
                ui.add_space(4.0);

                // 2. Настройки звука и PTT
                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(38, 41, 47))
                    .stroke(card_stroke)
                    .rounding(egui::Rounding::same(8.0))
                    .inner_margin(egui::Margin::same(12.0))
                    .show(ui, |ui| {
                        ui.set_width(full_width);
                        crate::widgets::audio_settings::draw_audio_settings(
                            ui,
                            &self.input_devices,
                            &self.output_devices,
                            &self.selected_input,
                            &self.selected_output,
                            &self.mixer,
                            &mut self.ptt_bind,
                        );
                        
                        crate::keybind::check_ptt_status(ui, &self.ptt_flag, self.ptt_bind, self.continuous_mode);
                        crate::widgets::ptt_button::draw_ptt_button(ui, &self.ptt_flag, &mut self.continuous_mode);
                    });

                ui.add_space(4.0);

                // 3. Список участников и комната
                egui::Frame::none()
                    .fill(egui::Color32::from_rgb(38, 41, 47))
                    .stroke(card_stroke)
                    .rounding(egui::Rounding::same(8.0))
                    .inner_margin(egui::Margin::same(12.0))
                    .show(ui, |ui| {
                        ui.set_width(full_width);
                        crate::widgets::room_panel::draw_room_panel(
                            ui,
                            &mut self.room_id_input,
                            &mut self.connect_ip_input,
                            &mut self.connect_port_input,
                            self.network.clone(),
                            self.my_id,
                            self.is_host.clone(),
                            self.room.clone(),
                            &self.mixer,
                            self.ptt_flag.load(std::sync::atomic::Ordering::Relaxed),
                        );
                    });
            });
        });

        ctx.request_repaint();
    }
}