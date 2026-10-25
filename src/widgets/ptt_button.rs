use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub fn draw_ptt_button(ui: &mut egui::Ui, ptt_flag: &Arc<AtomicBool>) {
    ui.separator();
    ui.horizontal(|ui| {
        // Чекбокс для постоянного включения микрофона
        let mut is_active = ptt_flag.load(Ordering::Relaxed);
        if ui.checkbox(&mut is_active, "🎙️ Микрофон активен (Постоянная передача)").changed() {
            ptt_flag.store(is_active, Ordering::Relaxed);
        }

        // Если постоянная передача не включена, доступна кнопка PTT
        if !is_active {
            let hold_btn = ui.button("Зажать для разговора (PTT)");
            if hold_btn.is_pointer_button_down_on() {
                ptt_flag.store(true, Ordering::Relaxed);
            } else if !ui.ctx().input(|i| i.pointer.any_down()) {
                ptt_flag.store(false, Ordering::Relaxed);
            }
        }
    });

    // Явная цветовая индикация состояния эфира
    if ptt_flag.load(Ordering::Relaxed) {
        ui.colored_label(egui::Color32::from_rgb(0, 255, 100), "● МИКРОФОН В ЭФИРЕ (Идет передача)");
    } else {
        ui.colored_label(egui::Color32::GRAY, "○ Микрофон выключен");
    }
}