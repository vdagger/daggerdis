use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub fn draw_ptt_button(ui: &mut egui::Ui, ptt_flag: &Arc<AtomicBool>) {
    let ptt_button = ui.button("НАЖМИ И ГОВОРИ");
    if ptt_button.clicked() {
        let current = ptt_flag.load(Ordering::Relaxed);
        ptt_flag.store(!current, Ordering::Relaxed);
    }
    if ptt_button.is_pointer_button_down_on() {
        ptt_flag.store(true, Ordering::Relaxed);
    } else if !ui.ctx().input(|i| i.pointer.any_down()) {
        ptt_flag.store(false, Ordering::Relaxed);
    }
}