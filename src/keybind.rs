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

/// Что решил шаг перехвата. `ptt` — только при смене состояния (иначе поток
/// каждый тик писал бы одно и то же), `mute_toggle` — фронт Ctrl+M.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HotkeyEvents {
    pub ptt: Option<bool>,
    pub mute_toggle: bool,
}

/// Решения по горячим клавишам без ОС и потоков.
///
/// Вход — то, что умеет прочитать любой бэкенд: «передача включена тумблером»,
/// «клавиша зажата», «зажата экранная зона», «нажат Ctrl», «нажат M». Win32
/// читает это через `GetAsyncKeyState`, X11/Wayland-бэкенд сможет читать из
/// `XGrabKey`/evdev и звать тот же `step` — поэтому логика про края и здесь,
/// а не в цикле потока.
#[derive(Clone, Copy, Debug, Default)]
pub struct HotkeyState {
    ptt: bool,
    mute_combo: bool,
}

impl HotkeyState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Один тик: накапливает фронты.
    pub fn step(
        &mut self,
        continuous: bool,
        key_down: bool,
        screen_ptt: bool,
        ctrl: bool,
        m: bool,
    ) -> HotkeyEvents {
        let active = continuous || key_down || screen_ptt;
        let mut ptt = None;
        if active != self.ptt {
            self.ptt = active;
            ptt = Some(active);
        }
        // Ctrl+M — по фронту: удержание не должно дёргать тумблер на каждом тике.
        let combo = ctrl && m;
        let mut mute_toggle = false;
        if combo && !self.mute_combo {
            mute_toggle = true;
        }
        self.mute_combo = combo;
        HotkeyEvents { ptt, mute_toggle }
    }
}

// По умолчанию Mouse 4 (боковая кнопка мыши 0x05)
pub static CURRENT_VK: AtomicI32 = AtomicI32::new(0x05);
static LISTENER_SPAWNED: AtomicBool = AtomicBool::new(false);
static IS_LISTENING: AtomicBool = AtomicBool::new(false);
static CONTINUOUS_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Экранная PTT-зона: суммируется с хоткеем, а не перетирает его
static SCREEN_PTT: AtomicBool = AtomicBool::new(false);
/// Глобальный мьют микрофона: Ctrl+M ловится тем же потоком, что и PTT.
/// GUI забирает событие через `take_mute_toggle` — сам он в этом потоке
/// не живёт, поэтому флаг, а не прямой вызов микшера.
static MUTE_EDGE: AtomicBool = AtomicBool::new(false);

/// Забирает накопившееся нажатие Ctrl+M (true — ровно одно нажатие).
pub fn take_mute_toggle() -> bool {
    MUTE_EDGE.swap(false, Ordering::Relaxed)
}

/// Вызывается из GUI: пользователь зажал/отпустил PTT-зону мышью
pub fn set_screen_ptt(active: bool) {
    SCREEN_PTT.store(active, Ordering::Relaxed);
}

/// Запускает независимый фоновый системный поток перехвата PTT в Windows
fn ensure_global_listener_spawned(ptt_flag: Arc<AtomicBool>) {
    // Вне Windows глобального перехвата нет: X11/Wayland требуют своей
    // реализации (XGrabKey/evdev), macOS — CGEventTap. Крутить пустой цикл
    // каждые 10 мс бессмысленно, поэтому честно говорим об этом один раз.
    #[cfg(not(windows))]
    {
        let _ = ptt_flag;
        if LISTENER_SPAWNED
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            println!(
                "[ХОТКЕЙ] Глобальный хоткей недоступен на этой платформе — \
                 PTT работает экранной кнопкой"
            );
        }
        return;
    }

    if LISTENER_SPAWNED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        std::thread::spawn(move || {
            println!("[ХОТКЕЙ] 🚀 Фоновый системный перехватчик PTT в играх запущен!");
            let mut state = HotkeyState::new();

            loop {
                std::thread::sleep(Duration::from_millis(10));

                let is_active = CONTINUOUS_ACTIVE.load(Ordering::Relaxed);

                #[cfg(windows)]
                {
                    let vk = CURRENT_VK.load(Ordering::Relaxed);
                    let key_down = if vk > 0 {
                        unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
                    } else {
                        false
                    };
                    let ctrl_down = unsafe { (GetAsyncKeyState(0x11) as u16 & 0x8000) != 0 };
                    let m_down = unsafe { (GetAsyncKeyState(0x4D) as u16 & 0x8000) != 0 };

                    // Решения — в чистой `HotkeyState`: тот же шаг сможет звать
                    // бэкенд X11/Wayland, когда появится.
                    let events = state.step(
                        is_active,
                        key_down,
                        SCREEN_PTT.load(Ordering::Relaxed),
                        ctrl_down,
                        m_down,
                    );
                    if let Some(on) = events.ptt {
                        ptt_flag.store(on, Ordering::Relaxed);
                        if on {
                            println!(
                                "[ХОТКЕЙ] 🎙️ ЭФИР ВКЛЮЧЕН (VK: {:#X}, Hands-Free: {})",
                                vk, is_active
                            );
                        } else {
                            println!("[ХОТКЕЙ] 🔇 ЭФИР ВЫКЛЮЧЕН");
                        }
                    }
                    if events.mute_toggle {
                        MUTE_EDGE.store(true, Ordering::Relaxed);
                    }
                }
            }
        });
    }
}

/// Возвращает понятное человеческое имя для кода клавиши Windows
#[cfg(windows)]
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

/// Короткая подпись назначенной кнопки — для подсказки на PTT-пилюле
pub fn shortcut_label(vk: i32) -> String {
    match vk {
        0x01 => "ЛКМ".to_string(),
        0x02 => "ПКМ".to_string(),
        0x04 => "КОЛЕСО".to_string(),
        0x05 => "MOUSE 4".to_string(),
        0x06 => "MOUSE 5".to_string(),
        0x14 => "CAPSLOCK".to_string(),
        0x20 => "ПРОБЕЛ".to_string(),
        0x10 | 0xA0 | 0xA1 => "SHIFT".to_string(),
        0x11 | 0xA2 | 0xA3 => "CTRL".to_string(),
        0x12 | 0xA4 | 0xA5 => "ALT".to_string(),
        0x09 => "TAB".to_string(),
        0xC0 => "~".to_string(),
        0x41..=0x5A => (vk as u8 as char).to_string(),
        0x70..=0x7B => format!("F{}", vk - 0x70 + 1),
        0 => "НЕ НАЗНАЧЕНО".to_string(),
        _ => format!("{:#X}", vk),
    }
}

/// Отрисовка кнопки назначения хоткея с фиксированной шириной
pub fn handle_keybind_ui(ui: &mut egui::Ui, current_bind: &mut Option<PttBind>, width: f32) {
    let is_listening = IS_LISTENING.load(Ordering::Relaxed);
    let vk = current_bind.unwrap_or_else(|| CURRENT_VK.load(Ordering::Relaxed));

    let text = if is_listening {
        "🔴 НАЖМИТЕ КЛАВИШУ...".to_string()
    } else {
        format!("PTT: {}", vk_to_name(vk))
    };

    let btn = ui.add_sized(
        [width, 22.0],
        egui::Button::new(egui::RichText::new(text).size(12.0)),
    );

    if btn.clicked() {
        IS_LISTENING.store(true, Ordering::Relaxed);
    }

    if is_listening {
        #[cfg(windows)]
        {
            unsafe {
                for code in 1..=254 {
                    // Игнорируем обычный клик ЛКМ при открытии меню
                    if code == 0x01 {
                        continue;
                    }
                    if (GetAsyncKeyState(code) as u16 & 0x8000) != 0 {
                        *current_bind = Some(code);
                        CURRENT_VK.store(code, Ordering::Relaxed);
                        IS_LISTENING.store(false, Ordering::Relaxed);
                        println!(
                            "[ХОТКЕЙ] Назначена новая кнопка PTT: {} (код {:#X})",
                            vk_to_name(code),
                            code
                        );
                        break;
                    }
                }
            }
        }
    }
}

/// Поддерживает работу слушателя и синхронизирует флаги
pub fn check_ptt_status(
    _ctx: &egui::Context,
    ptt_flag: &Arc<AtomicBool>,
    bind: Option<PttBind>,
    continuous_mode: bool,
) {
    ensure_global_listener_spawned(ptt_flag.clone());
    CONTINUOUS_ACTIVE.store(continuous_mode, Ordering::Relaxed);

    if let Some(vk) = bind {
        CURRENT_VK.store(vk, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ptt_reacts_to_edges_only() {
        let mut st = HotkeyState::new();
        // Клавиша нажата — событие включения.
        assert_eq!(
            st.step(false, true, false, false, false).ptt,
            Some(true),
            "первое нажатие включает эфир"
        );
        // Держим — повторных событий нет.
        assert_eq!(st.step(false, true, false, false, false).ptt, None);
        // Отпустили — событие выключения.
        assert_eq!(st.step(false, false, false, false, false).ptt, Some(false));
        assert_eq!(st.step(false, false, false, false, false).ptt, None);
    }

    #[test]
    fn ptt_sums_hands_free_key_and_screen_zone() {
        let mut st = HotkeyState::new();
        // Тумблер включён — эфир уже идёт.
        assert_eq!(st.step(true, false, false, false, false).ptt, Some(true));
        // Отпустили клавишу, но тумблер держит передачу: выключения нет.
        assert_eq!(st.step(true, false, false, false, false).ptt, None);
        // Экранная зона дожимает, когда тумблер выключили.
        assert_eq!(st.step(false, false, true, false, false).ptt, None);
        assert_eq!(st.step(false, false, false, false, false).ptt, Some(false));
    }

    #[test]
    fn mute_toggles_once_per_ctrl_m_press() {
        let mut st = HotkeyState::new();
        assert!(st.step(false, false, false, true, true).mute_toggle, "первое нажатие");
        assert!(!st.step(false, false, false, true, true).mute_toggle, "удержание молчит");
        assert!(!st.step(false, false, false, true, false).mute_toggle);
        assert!(st.step(false, false, false, true, true).mute_toggle, "второе нажатие");
    }

    #[test]
    fn mute_needs_both_keys() {
        let mut st = HotkeyState::new();
        // Ctrl без M и M без Ctrl — не мьют.
        assert!(!st.step(false, false, false, true, false).mute_toggle);
        assert!(!st.step(false, false, false, false, true).mute_toggle);
    }
}
