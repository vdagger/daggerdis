//! Управление биндами PTT (Push-to-Talk)

use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Универсальное перечисление для бинда PTT
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PttBind {
    Keyboard(eframe::egui::Key),
    Mouse(eframe::egui::PointerButton),
}

/// Обновляет UI для выбора бинда PTT
pub fn handle_keybind_ui(ui: &mut eframe::egui::Ui, current_bind: &mut Option<PttBind>) {
    let text = match current_bind {
        Some(PttBind::Keyboard(k)) => format!("PTT: Клавиша {:?}", k),
        Some(PttBind::Mouse(b)) => format!("PTT: Мышь {:?}", b),
        None => "Нажмите любую клавишу или кнопку мыши...".to_string(),
    };
    if ui.button(text).clicked() { *current_bind = None; }

    if current_bind.is_none() {
        // Опрос клавиатуры
        if let Some(k) = ui.ctx().input(|i| i.keys_down.iter().next().cloned()) {
            *current_bind = Some(PttBind::Keyboard(k));
        }
        // Опрос мыши (ЛКМ, ПКМ, Колесико, Боковые кнопки)
        for button in [
            eframe::egui::PointerButton::Primary,
            eframe::egui::PointerButton::Secondary,
            eframe::egui::PointerButton::Middle,
            eframe::egui::PointerButton::Extra1,
            eframe::egui::PointerButton::Extra2,
        ] {
            if ui.ctx().input(|i| i.pointer.button_down(button)) {
                *current_bind = Some(PttBind::Mouse(button));
            }
        }
    }
}

/// Проверяет статус PTT по бинду
pub fn check_ptt_status(ui: &eframe::egui::Ui, ptt_flag: &Arc<AtomicBool>, bind: Option<PttBind>) {
    let is_down = match bind {
        Some(PttBind::Keyboard(k)) => ui.ctx().input(|i| i.key_down(k)),
        Some(PttBind::Mouse(b)) => ui.ctx().input(|i| i.pointer.button_down(b)),
        None => false,
    };
    ptt_flag.store(is_down, Ordering::Relaxed);
}