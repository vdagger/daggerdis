//! Список окон для трансляции окна вместо всего экрана.
//!
//! Захват самого окна через Windows Graphics Capture потребовал бы второго
//! пайплайна (своя текстура, свой конвертер); пока берём окно как **область**
//! кадра рабочего стола: видеопроцессор вырезает его прямоугольник и отдаёт
//! NV12 уже нужного размера. Качество то же (масштаба нет), битрейт — по числу
//! пикселей окна. Ограничение честное: окно должно быть видно на экране и не
//! перекрыто другим — тогда в кадр попадёт перекрывающее окно.

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, GWL_EXSTYLE,
    WS_EX_TOOLWINDOW,
};

/// Окно, которое можно транслировать.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub title: String,
    /// Размер по рамке окна (то, что реально видно на экране).
    pub width: u32,
    pub height: u32,
}

/// Окна верхнего уровня, которые имеет смысл предлагать: видимые, не
/// свёрнутые, с заголовком, не служебные и не свои собственные.
pub fn list_windows() -> Vec<WindowInfo> {
    let mut found: Vec<WindowInfo> = Vec::new();
    let own = std::process::id();
    unsafe {
        let _ = EnumWindows(
            Some(enum_proc),
            windows::Win32::Foundation::LPARAM(&mut found as *mut Vec<WindowInfo> as isize),
        );
    }
    found.retain(|w| w.hwnd as u32 != 0);
    // Свои окна из списка убираем: транслировать интерфейс самого Albus
    // бессмысленно (и это рекурсия превью).
    found.retain(|w| {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(HWND(w.hwnd as *mut _), Some(&mut pid)) };
        pid != own
    });
    found.sort_by_key(|a| a.title.to_lowercase());
    found
}

unsafe extern "system" fn enum_proc(
    hwnd: HWND,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::core::BOOL {
    let list = unsafe { &mut *(lparam.0 as *mut Vec<WindowInfo>) };
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() || unsafe { IsIconic(hwnd) }.as_bool() {
        return windows::core::BOOL(1);
    }
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    if style & WS_EX_TOOLWINDOW.0 != 0 {
        return windows::core::BOOL(1);
    }
    let len = unsafe { GetWindowTextLengthW(hwnd) };
    if len <= 0 {
        return windows::core::BOOL(1);
    }
    let mut buf = vec![0u16; len as usize + 1];
    let written = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if written <= 0 {
        return windows::core::BOOL(1);
    }
    let title = String::from_utf16_lossy(&buf[..written as usize]);
    if title.trim().is_empty() {
        return windows::core::BOOL(1);
    }
    // Классы-оболочки рабочего стола («Progman», «WorkerW») не транслируем:
    // это и есть весь экран, только без остальных окон.
    let mut class_buf = [0u16; 128];
    let class_len = unsafe { GetClassNameW(hwnd, &mut class_buf) };
    let class = String::from_utf16_lossy(&class_buf[..class_len.max(0) as usize]);
    if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd") {
        return windows::core::BOOL(1);
    }
    let Some((w, h)) = (unsafe { window_size(hwnd) }) else {
        return windows::core::BOOL(1);
    };
    if w < 64 || h < 64 {
        return windows::core::BOOL(1);
    }
    list.push(WindowInfo {
        hwnd: hwnd.0 as isize,
        title,
        width: w,
        height: h,
    });
    windows::core::BOOL(1)
}

/// Прямоугольник окна на экране: `DWM` отдаёт его без тени, обычный
/// `GetWindowRect` — с ней, и кадр выходил бы с полями.
pub fn window_rect(hwnd: isize) -> Option<(i32, i32, u32, u32)> {
    let h = HWND(hwnd as *mut _);
    unsafe {
        let mut rect = RECT::default();
        let size = std::mem::size_of::<RECT>() as u32;
        let dwm = DwmGetWindowAttribute(
            h,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rect as *mut _ as *mut _,
            size,
        );
        if dwm.is_err() && GetWindowRect(h, &mut rect).is_err() {
            return None;
        }
        let w = (rect.right - rect.left).max(0) as u32;
        let hh = (rect.bottom - rect.top).max(0) as u32;
        if w == 0 || hh == 0 {
            return None;
        }
        Some((rect.left, rect.top, w, hh))
    }
}

unsafe fn window_size(hwnd: HWND) -> Option<(u32, u32)> {
    let h = hwnd.0 as isize;
    let (_, _, w, hh) = window_rect(h)?;
    Some((w, hh))
}

/// Пересечение окна с экраном, приведённое к чётным координатам и размерам.
///
/// NV12 и H.264 требуют чётного размера; окно может вылезать за экран или
/// быть шире него — тогда берём видимую часть. `None` — от окна ничего не
/// осталось (свернуто, уведено за край).
pub fn clamp_rect(
    rect: (i32, i32, u32, u32),
    screen: (u32, u32),
) -> Option<(i32, i32, u32, u32)> {
    let (x, y, w, h) = rect;
    let sw = screen.0 as i32;
    let sh = screen.1 as i32;
    let left = x.max(0);
    let top = y.max(0);
    let right = (x + w as i32).min(sw);
    let bottom = (y + h as i32).min(sh);
    let width = right - left;
    let height = bottom - top;
    if width < 32 || height < 32 {
        return None;
    }
    let left = left & !1;
    let top = top & !1;
    let width = (width as u32) & !1;
    let height = (height as u32) & !1;
    Some((left, top, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_crops_window_to_screen() {
        // окно целиком на экране
        assert_eq!(
            clamp_rect((100, 50, 800, 600), (1920, 1080)),
            Some((100, 50, 800, 600))
        );
        // левый верхний угол за экраном — отрезаем
        assert_eq!(
            clamp_rect((-100, -40, 800, 600), (1920, 1080)),
            Some((0, 0, 700, 560))
        );
        // окно шире экрана — по экрану
        assert_eq!(
            clamp_rect((0, 0, 4000, 3000), (1920, 1080)),
            Some((0, 0, 1920, 1080))
        );
        // нечётные координаты и размеры — к чётным
        assert_eq!(
            clamp_rect((101, 51, 801, 601), (1920, 1080)),
            Some((100, 50, 800, 600))
        );
        // окно уведено за край — захватывать нечего
        assert_eq!(clamp_rect((2000, 0, 400, 300), (1920, 1080)), None);
        // слишком маленькое — не стоит отдельного потока
        assert_eq!(clamp_rect((10, 10, 20, 20), (1920, 1080)), None);
    }
}
