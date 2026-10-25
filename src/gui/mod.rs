//! Графический интерфейс для Discord-рации

mod stream_control;

use eframe::egui;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::sync::Mutex;

/// Основное приложение графического интерфейса
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
    playback_rx: Arc<std::sync::Mutex<tokio::sync::mpsc::Receiver<Vec<u8>>>>,
    prev_output_index: std::sync::atomic::AtomicU32,
}

impl RadioApp {
    /// Создает новое приложение с заданным микшером и флагом PTT
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
        playback_rx: Arc<std::sync::Mutex<tokio::sync::mpsc::Receiver<Vec<u8>>>>,
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
        }
    }
}

impl eframe::App for RadioApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Обновляем внешний IP из ext_ip
        {
            if let Ok(ip_lock) = self.ext_ip.try_lock() {
                self.my_external_ip = ip_lock.clone();
            }
        }

        // Проверяем изменение индекса входного устройства и перезапускаем поток при необходимости
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

        // Проверяем изменение индекса выходного устройства и перезапускаем поток воспроизведения при необходимости
        let current_output_index = self.selected_output.load(std::sync::atomic::Ordering::Relaxed);
        let prev_output_index = self.prev_output_index.load(std::sync::atomic::Ordering::Relaxed);
        if current_output_index != prev_output_index {
            if let Some(device_name) = self.output_devices.get(current_output_index as usize) {
                stream_control::restart_playback_stream(
                    &self.playback_stream,
                    self.playback_rx.clone(),
                    device_name.clone(),
                );
            }
            self.prev_output_index.store(current_output_index, std::sync::atomic::Ordering::Relaxed);
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            crate::widgets::info_panel::draw_info_panel(ui, self.my_id, &self.my_external_ip);
            
            crate::widgets::audio_settings::draw_audio_settings(
                ui,
                &self.input_devices,
                &self.output_devices,
                &self.selected_input,
                &self.selected_output,
                &self.mixer,
            );
            
            crate::keybind::handle_keybind_ui(ui, &mut self.ptt_bind);
            crate::keybind::check_ptt_status(ui, &self.ptt_flag, self.ptt_bind);
            crate::widgets::ptt_button::draw_ptt_button(ui, &self.ptt_flag);
            crate::widgets::room_panel::draw_room_panel(
                ui,
                &mut self.room_id_input,
                &mut self.connect_ip_input,
                &mut self.connect_port_input,
                self.network.clone(),
                self.my_id,
                self.is_host.clone(),
                self.room.clone(),
            );
        });

        // Запрос на перерисовку для плавной анимации
        ctx.request_repaint();
    }
}