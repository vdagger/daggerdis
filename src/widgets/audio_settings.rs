//! Настройки аудио с ровной двухколоночной сеткой

use eframe::egui;

fn truncate_name(name: &str, max_chars: usize) -> String {
    if name.chars().count() > max_chars {
        let mut truncated: String = name.chars().take(max_chars.saturating_sub(2)).collect();
        truncated.push_str("..");
        truncated
    } else {
        name.to_string()
    }
}

pub fn draw_audio_settings(
    ui: &mut eframe::egui::Ui,
    input_devices: &[String],
    output_devices: &[String],
    selected_input: &std::sync::Arc<std::sync::atomic::AtomicU32>,
    selected_output: &std::sync::Arc<std::sync::atomic::AtomicU32>,
    mixer: &crate::mixer::AudioMixer,
    ptt_bind: &mut Option<crate::keybind::PttBind>,
) {
    // 1. VU-Meter на всю ширину блока
    ui.add(
        egui::ProgressBar::new(mixer.get_input_peak())
            .text(egui::RichText::new("Микрофон (VU-Meter)").size(11.0).color(egui::Color32::WHITE))
            .desired_width(ui.available_width())
    );

    ui.add_space(8.0);

    const CONTROL_WIDTH: f32 = 210.0;
    let label_color = egui::Color32::from_rgb(205, 210, 220); // Четкий читаемый цвет подписей

    egui::Grid::new("audio_settings_grid")
        .num_columns(2)
        .spacing([14.0, 8.0])
        .show(ui, |ui| {
            // Ряд 1: Микрофон (Канал 1)
            let mut ch1_vol = mixer.ch1_volume();
            if ui.add_sized(
                [CONTROL_WIDTH, 20.0],
                egui::Slider::new(&mut ch1_vol, 0..=100).show_value(true)
            ).changed() {
                mixer.set_ch1_volume(ch1_vol);
            }
            ui.label(egui::RichText::new("Громкость Входа 1 (Микрофон)").color(label_color));
            ui.end_row();

            // Ряд 2: Гитара (Канал 2)
            let mut ch2_vol = mixer.ch2_volume();
            if ui.add_sized(
                [CONTROL_WIDTH, 20.0],
                egui::Slider::new(&mut ch2_vol, 0..=100).show_value(true)
            ).changed() {
                mixer.set_ch2_volume(ch2_vol);
            }
            ui.label(egui::RichText::new("Громкость Входа 2 (Гитара)").color(label_color));
            ui.end_row();

            // Ряд 3: Громкость собеседников (Выход до 300%)
            let mut out_vol = mixer.output_volume();
            if ui.add_sized(
                [CONTROL_WIDTH, 20.0],
                egui::Slider::new(&mut out_vol, 0..=300).suffix("%").show_value(true)
            ).changed() {
                mixer.set_output_volume(out_vol);
            }
            ui.label(egui::RichText::new("🔊 Громкость собеседников (Выход)").color(label_color));
            ui.end_row();

            // Ряд 4: Входное устройство
            let in_idx = selected_input.load(std::sync::atomic::Ordering::Relaxed) as usize;
            let full_in_name = input_devices.get(in_idx).cloned().unwrap_or_else(|| "Не выбрано".to_string());
            let short_in_name = truncate_name(&full_in_name, 22);

            let in_combo = egui::ComboBox::from_id_source("input_device_combo")
                .width(CONTROL_WIDTH)
                .selected_text(short_in_name)
                .show_ui(ui, |ui| {
                    for (i, device) in input_devices.iter().enumerate() {
                        let mut current_idx = selected_input.load(std::sync::atomic::Ordering::Relaxed);
                        if ui.selectable_value(&mut current_idx, i as u32, device).changed() {
                            selected_input.store(current_idx, std::sync::atomic::Ordering::Relaxed);
                            println!("[АУДИО] Выбран микрофон: {}", device);
                        }
                    }
                });
            in_combo.response.on_hover_text(&full_in_name);
            ui.label(egui::RichText::new("Входное устройство").color(label_color));
            ui.end_row();

            // Ряд 5: Выходное устройство
            let out_idx = selected_output.load(std::sync::atomic::Ordering::Relaxed) as usize;
            let full_out_name = output_devices.get(out_idx).cloned().unwrap_or_else(|| "Не выбрано".to_string());
            let short_out_name = truncate_name(&full_out_name, 22);

            let out_combo = egui::ComboBox::from_id_source("output_device_combo")
                .width(CONTROL_WIDTH)
                .selected_text(short_out_name)
                .show_ui(ui, |ui| {
                    for (i, device) in output_devices.iter().enumerate() {
                        let mut current_idx = selected_output.load(std::sync::atomic::Ordering::Relaxed);
                        if ui.selectable_value(&mut current_idx, i as u32, device).changed() {
                            selected_output.store(current_idx, std::sync::atomic::Ordering::Relaxed);
                            println!("[АУДИО] Выбраны динамики: {}", device);
                        }
                    }
                });
            out_combo.response.on_hover_text(&full_out_name);
            ui.label(egui::RichText::new("Выходное устройство").color(label_color));
            ui.end_row();

            // Ряд 6: Кнопка PTT
            crate::keybind::handle_keybind_ui(ui, ptt_bind, CONTROL_WIDTH);
            ui.label(egui::RichText::new("Клавиша рации (PTT)").color(label_color));
            ui.end_row();
        });
}