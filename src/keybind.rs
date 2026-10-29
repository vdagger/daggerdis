//! Универсальный модуль глобального перехвата PTT через Win32 API

use eframe::egui;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[cfg(windows)]
extern "system" {
    fn GetAsyncKeyState(vKey: i32) -> i16;
}

/// Системный код клавиши Windows (VK)
pub type PttBind = i32;

// По умолчанию Mouse 4 (боковая кнопка мыши 0x05)
pub static CURRENT_VK: AtomicI32 = AtomicI32::new(0x05); 
static LISTENER_SPAWNED: AtomicBool = AtomicBool::new(false);
static IS_LISTENING: AtomicBool = AtomicBool::new(false);
static CONTINUOUS_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Запускает независимый фоновый системный поток перехвата PTT в Windows
fn ensure_global_listener_spawned(ptt_flag: Arc<AtomicBool>) {
    if LISTENER_SPAWNED.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        std::thread::spawn(move || {
            println!("[ХОТКЕЙ] 🚀 Фоновый системный перехватчик PTT в играх запущен!");
            let mut last_state = false;

            loop {
                std::thread::sleep(Duration::from_millis(10));

                if CONTINUOUS_ACTIVE.load(Ordering::Relaxed) {
                    ptt_flag.store(true, Ordering::Relaxed);
                    continue;
                }

                #[cfg(windows)]
                {
                    let vk = CURRENT_VK.load(Ordering::Relaxed);
                    if vk > 0 {
                        unsafe {
                            // Проверяем старший бит (физически зажата ли клавиша)
                            let is_down = (GetAsyncKeyState(vk) as u16 & 0x8000) != 0;
                            if is_down != last_state {
                                last_state = is_down;
                                ptt_flag.store(is_down, Ordering::Relaxed);
                                if is_down {
                                    println!("[ХОТКЕЙ] 🎙️ PTT НАЖАТ (VK: {:#X})", vk);
                                } else {
                                    println!("[ХОТКЕЙ] 🔇 PTT ОТПУЩЕН (VK: {:#X})", vk);
                                }
                            }
                        }
                    }
                }
            }
        });
    }
}

/// Возвращает понятное человеческое имя для кода клавиши Windows
fn vk_to_name(vk: i32) -> String {
    match vk {
        0x01 => "Мышь: ЛКМ".to_string(),
        0x02 => "Мышь: ПКМ".to_string(),
        0x04 => "Мышь: Колесико".to_string(),
        0x05 => "Мышь: Боковая 1 (Mouse 4)".to_string(),
        0x06 => "Мышь: Боковая 2 (Mouse 5)".to_string(),
        0x14 => "Клавиша: CapsLock".to_string(),
        0x20 => "Клавиша: Пробел".to_string(),
        0x10 | 0xA0 | 0xA1 => "Клавиша: Shift".to_string(),
        0x11 | 0xA2 | 0xA3 => "Клавиша: Ctrl".to_string(),
        0x12 | 0xA4 | 0xA5 => "Клавиша: Alt".to_string(),
        0x09 => "Клавиша: Tab".to_string(),
        0xC0 => "Клавиша: Тильда (~)".to_string(),
        0x41..=0x5A => format!("Клавиша: {}", (vk as u8 as char)),
        0x70..=0x7B => format!("Клавиша: F{}", vk - 0x70 + 1),
        0 => "Не назначено".to_string(),
        _ => format!("Кнопка (Код {:#X})", vk),
    }
}

/// Отрисовка кнопки назначения хоткея
pub fn handle_keybind_ui(ui: &mut egui::Ui, current_bind: &mut Option<PttBind>) {
    let is_listening = IS_LISTENING.load(Ordering::Relaxed);
    let vk = current_bind.unwrap_or_else(|| CURRENT_VK.load(Ordering::Relaxed));

    let text = if is_listening {
        "🔴 НАЖМИТЕ ЛЮБУЮ КНОПКУ МЫШИ ИЛИ КЛАВИАТУРЫ...".to_string()
    } else {
        format!("PTT: {}", vk_to_name(vk))
    };

    if ui.button(text).clicked() {
        IS_LISTENING.store(true, Ordering::Relaxed);
    }

    if is_listening {
        #[cfg(windows)]
        {
            unsafe {
                for code in 1..=254 {
                    // Игнорируем ЛКМ чтобы не биндить сам клик
                    if code == 0x01 {
                        continue;
                    }
                    if (GetAsyncKeyState(code) as u16 & 0x8000) != 0 {
                        *current_bind = Some(code);
                        CURRENT_VK.store(code, Ordering::Relaxed);
                        IS_LISTENING.store(false, Ordering::Relaxed);
                        println!("[ХОТКЕЙ] Назначена новая кнопка PTT: {} (код {:#X})", vk_to_name(code), code);
                        break;
                    }
                }
            }
        }
    }
}

/// Поддерживает работу слушателя и передает флаг
pub fn check_ptt_status(_ui: &egui::Ui, ptt_flag: &Arc<AtomicBool>, bind: Option<PttBind>, continuous_mode: bool) {
    ensure_global_listener_spawned(ptt_flag.clone());
    CONTINUOUS_ACTIVE.store(continuous_mode, Ordering::Relaxed);

    if let Some(vk) = bind {
        CURRENT_VK.store(vk, Ordering::Relaxed);
    }
}