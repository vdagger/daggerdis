use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub fn draw_ptt_button(ui: &mut egui::Ui, ptt_flag: &Arc<AtomicBool>, continuous_mode: &mut bool) {
    ui.separator();
    ui.horizontal(|ui| {
        // Интерактивный тумблер постоянной передачи
        if ui.checkbox(continuous_mode, "🎙️ Микрофон активен (Постоянная передача)").changed() {
            if *continuous_mode {
                ptt_flag.store(true, Ordering::Relaxed);
            } else {
                ptt_flag.store(false, Ordering::Relaxed);
            }
        }

        // Кнопка PTT доступна, только если постоянная передача не включена
        if !*continuous_mode {
            let hold_btn = ui.button("Зажать для разговора (PTT)");
            if hold_btn.is_pointer_button_down_on() {
                ptt_flag.store(true, Ordering::Relaxed);
            } else if !ui.ctx().input(|i| i.pointer.any_down()) {
                ptt_flag.store(false, Ordering::Relaxed);
            }
        }
    });

    // Цветовая индикация состояния эфира
    if ptt_flag.load(Ordering::Relaxed) {
        ui.colored_label(egui::Color32::from_rgb(0, 255, 100), "● МИКРОФОН В ЭФИРЕ (Идет передача)");
    } else {
        ui.colored_label(egui::Color32::GRAY, "○ Микрофон выключен");
    }
}