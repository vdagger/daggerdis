//! Настройки аудио

use eframe::egui;

/// Отрисовка настроек аудио
pub fn draw_audio_settings(
    ui: &mut eframe::egui::Ui,
    input_devices: &[String],
    output_devices: &[String],
    selected_input: &std::sync::Arc<std::sync::atomic::AtomicU32>,
    selected_output: &std::sync::Arc<std::sync::atomic::AtomicU32>,
    mixer: &crate::mixer::AudioMixer,
) {
    // Отрисовка VU-Meter
    ui.add(egui::ProgressBar::new(mixer.get_input_peak()).text("Микрофон (VU-Meter)"));

    // Слайдер для Канала 1 (Микрофон)
    let mut ch1_vol = mixer.ch1_volume();
    if ui.add(egui::Slider::new(&mut ch1_vol, 0..=100).text("Громкость Входа 1 (Микрофон)")).changed() {
        mixer.set_ch1_volume(ch1_vol);
    }

    // Слайдер для Канала 2 (Гитара / Line-In)
    let mut ch2_vol = mixer.ch2_volume();
    if ui.add(egui::Slider::new(&mut ch2_vol, 0..=100).text("Громкость Входа 2 (Гитара)")).changed() {
        mixer.set_ch2_volume(ch2_vol);
    }

    // Отрисовка Входных устройств
    egui::ComboBox::from_label("Входное устройство")
        .selected_text(input_devices.get(selected_input.load(std::sync::atomic::Ordering::Relaxed) as usize)
            .unwrap_or(&"Не выбрано".to_string()))
        .show_ui(ui, |ui| {
            for (i, device) in input_devices.iter().enumerate() {
                let mut current_idx = selected_input.load(std::sync::atomic::Ordering::Relaxed);
                if ui.selectable_value(&mut current_idx, i as u32, device).changed() {
                    selected_input.store(current_idx, std::sync::atomic::Ordering::Relaxed);
                    println!("[АУДИО] Выбран микрофон с индексом: {}", i);
                }
            }
        });

    // Отрисовка Выходных устройств
    egui::ComboBox::from_label("Выходное устройство")
        .selected_text(output_devices.get(selected_output.load(std::sync::atomic::Ordering::Relaxed) as usize)
            .unwrap_or(&"Не выбрано".to_string()))
        .show_ui(ui, |ui| {
            for (i, device) in output_devices.iter().enumerate() {
                let mut current_idx = selected_output.load(std::sync::atomic::Ordering::Relaxed);
                if ui.selectable_value(&mut current_idx, i as u32, device).changed() {
                    selected_output.store(current_idx, std::sync::atomic::Ordering::Relaxed);
                    println!("[АУДИО] Выбраны динамики с индексом: {}", i);
                }
            }
        });
}