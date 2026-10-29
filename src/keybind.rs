//! Управление биндами PTT (Push-to-Talk) с поддержкой глобального перехвата в играх

use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Универсальное перечисление для бинда PTT
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PttBind {
    Keyboard(eframe::egui::Key),
    Mouse(eframe::egui::PointerButton),
}

#[cfg(windows)]
extern "system" {
    fn GetAsyncKeyState(vKey: i32) -> i16;
}

/// Преобразует бинд egui в системный виртуальный код клавиши Windows (Virtual Key Code)
fn ptt_bind_to_vk(bind: PttBind) -> Option<i32> {
    match bind {
        PttBind::Mouse(eframe::egui::PointerButton::Primary) => Some(0x01),   // ЛКМ
        PttBind::Mouse(eframe::egui::PointerButton::Secondary) => Some(0x02), // ПКМ
        PttBind::Mouse(eframe::egui::PointerButton::Middle) => Some(0x04),    // Колесико
        PttBind::Mouse(eframe::egui::PointerButton::Extra1) => Some(0x05),    // Боковая мышь 1 (Mouse 4)
        PttBind::Mouse(eframe::egui::PointerButton::Extra2) => Some(0x06),    // Боковая мышь 2 (Mouse 5)
        PttBind::Keyboard(k) => match k {
            eframe::egui::Key::Space => Some(0x20),
            eframe::egui::Key::Tab => Some(0x09),
            eframe::egui::Key::Enter => Some(0x0D),
            eframe::egui::Key::Escape => Some(0x1B),
            eframe::egui::Key::Backspace => Some(0x08),
            // Буквы A-Z (0x41 .. 0x5A)
            eframe::egui::Key::A => Some(0x41),
            eframe::egui::Key::B => Some(0x42),
            eframe::egui::Key::C => Some(0x43),
            eframe::egui::Key::D => Some(0x44),
            eframe::egui::Key::E => Some(0x45),
            eframe::egui::Key::F => Some(0x46),
            eframe::egui::Key::G => Some(0x47),
            eframe::egui::Key::H => Some(0x48),
            eframe::egui::Key::I => Some(0x49),
            eframe::egui::Key::J => Some(0x4A),
            eframe::egui::Key::K => Some(0x4B),
            eframe::egui::Key::L => Some(0x4C),
            eframe::egui::Key::M => Some(0x4D),
            eframe::egui::Key::N => Some(0x4E),
            eframe::egui::Key::O => Some(0x4F),
            eframe::egui::Key::P => Some(0x50),
            eframe::egui::Key::Q => Some(0x51),
            eframe::egui::Key::R => Some(0x52),
            eframe::egui::Key::S => Some(0x53),
            eframe::egui::Key::T => Some(0x54),
            eframe::egui::Key::U => Some(0x55),
            eframe::egui::Key::V => Some(0x56),
            eframe::egui::Key::W => Some(0x57),
            eframe::egui::Key::X => Some(0x58),
            eframe::egui::Key::Y => Some(0x59),
            eframe::egui::Key::Z => Some(0x5A),
            // Цифры 0-9 (0x30 .. 0x39)
            eframe::egui::Key::Num0 => Some(0x30),
            eframe::egui::Key::Num1 => Some(0x31),
            eframe::egui::Key::Num2 => Some(0x32),
            eframe::egui::Key::Num3 => Some(0x33),
            eframe::egui::Key::Num4 => Some(0x34),
            eframe::egui::Key::Num5 => Some(0x35),
            eframe::egui::Key::Num6 => Some(0x36),
            eframe::egui::Key::Num7 => Some(0x37),
            eframe::egui::Key::Num8 => Some(0x38),
            eframe::egui::Key::Num9 => Some(0x39),
            // Функциональные клавиши F1-F12
            eframe::egui::Key::F1 => Some(0x70),
            eframe::egui::Key::F2 => Some(0x71),
            eframe::egui::Key::F3 => Some(0x72),
            eframe::egui::Key::F4 => Some(0x73),
            eframe::egui::Key::F5 => Some(0x74),
            eframe::egui::Key::F6 => Some(0x75),
            eframe::egui::Key::F7 => Some(0x76),
            eframe::egui::Key::F8 => Some(0x77),
            eframe::egui::Key::F9 => Some(0x78),
            eframe::egui::Key::F10 => Some(0x79),
            eframe::egui::Key::F11 => Some(0x7A),
            eframe::egui::Key::F12 => Some(0x7B),
            _ => None,
        },
    }
}

/// Обновляет UI для выбора бинда PTT
pub fn handle_keybind_ui(ui: &mut eframe::egui::Ui, current_bind: &mut Option<PttBind>) {
    let text = match current_bind {
        Some(PttBind::Keyboard(k)) => format!("PTT: Клавиша {:?}", k),
        Some(PttBind::Mouse(b)) => format!("PTT: Мышь {:?}", b),
        None => "Нажмите любую клавишу или кнопку мыши...".to_string(),
    };
    if ui.button(text).clicked() { 
        *current_bind = None; 
    }

    if current_bind.is_none() {
        // Опрос клавиатуры egui
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

/// Проверяет статус нажатия PTT глобально во всей системе (даже если окно не в фокусе)
pub fn check_ptt_status(ui: &eframe::egui::Ui, ptt_flag: &Arc<AtomicBool>, bind: Option<PttBind>, continuous_mode: bool) {
    if continuous_mode {
        ptt_flag.store(true, Ordering::Relaxed);
        return;
    }

    // 1. Глобальная проверка состояния клавиши через Windows API (работает в свернутом виде и в играх)
    #[cfg(windows)]
    if let Some(b) = bind {
        if let Some(vk) = ptt_bind_to_vk(b) {
            unsafe {
                if (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 {
                    ptt_flag.store(true, Ordering::Relaxed);
                    return;
                }
            }
        }
    }

    // 2. Резервная проверка через egui (когда окно активно)
    let is_down = match bind {
        Some(PttBind::Keyboard(k)) => ui.ctx().input(|i| i.key_down(k)),
        Some(PttBind::Mouse(b)) => ui.ctx().input(|i| i.pointer.button_down(b)),
        None => false,
    };
    ptt_flag.store(is_down, Ordering::Relaxed);
}