use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub fn draw_ptt_button(ui: &mut egui::Ui, ptt_flag: &Arc<AtomicBool>, continuous_mode: &mut bool) {
    ui.separator();
    ui.horizontal(|ui| {
        // Чекбокс постоянной передачи (чистый белый читаемый текст без битых эмодзи)
        let cb_text = egui::RichText::new("Постоянная передача (Hands-Free)")
            .color(egui::Color32::from_rgb(230, 235, 245));
            
        if ui.checkbox(continuous_mode, cb_text).changed() {
            ptt_flag.store(*continuous_mode, Ordering::Relaxed);
        }

        // Экранная кнопка PTT
        if !*continuous_mode {
            let btn_text = egui::RichText::new("Зажать для разговора (PTT)")
                .strong()
                .color(egui::Color32::WHITE);
            let hold_btn = ui.add(egui::Button::new(btn_text));
            
            if hold_btn.is_pointer_button_down_on() {
                ptt_flag.store(true, Ordering::Relaxed);
            } else if hold_btn.hovered() && !ui.ctx().input(|i| i.pointer.any_down()) {
                ptt_flag.store(false, Ordering::Relaxed);
            }
        }
    });

    // Индикация эфира: мягкий серый в покое / неоновый зеленый при речи
    if ptt_flag.load(Ordering::Relaxed) {
        let label = egui::RichText::new("● МИКРОФОН В ЭФИРЕ (Идет передача)")
            .strong()
            .color(egui::Color32::from_rgb(34, 197, 94)); // Приятный изумрудный
        ui.label(label);
    } else {
        let label = egui::RichText::new("○ Микрофон выключен")
            .color(egui::Color32::from_rgb(160, 166, 178)); // Четкий читаемый нейтральный серый
        ui.label(label);
    }
}