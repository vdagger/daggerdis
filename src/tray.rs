//! Иконка в системном трее и уведомления Windows.
//!
//! Зачем свой поток: `Shell_NotifyIcon` требует окно, которое принимает
//! сообщения от оболочки (клик по иконке, выбор пункта меню). Окно создаётся
//! в отдельном потоке со своим циклом сообщений — GUI-поток eframe занят
//! отрисовкой, и вешать туда оконную процедуру нельзя.
//!
//! Связь с GUI — через атомики: поток трея только выставляет «просят показать»
//! или «просят выйти», а окно и жизненный цикл приложения остаются за GUI.
//! Обратный путь (уведомление) идёт через [`notify`], её можно звать из любого
//! потока.

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HWND, HINSTANCE, LPARAM, LRESULT, POINT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::Shell::{
        ExtractIconW, Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD,
        NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        AppendMenuW, CreatePopupMenu, CreateWindowExW, DefWindowProcW, DestroyMenu, DispatchMessageW,
        GetCursorPos, GetMessageW, LoadIconW, PostMessageW, PostQuitMessage, RegisterClassW,
        SetForegroundWindow, ShowWindow, TrackPopupMenu, TranslateMessage, SW_SHOW, WM_CLOSE,
        IDI_APPLICATION, MF_STRING, MSG, TPM_BOTTOMALIGN, TPM_RIGHTBUTTON, WINDOW_EX_STYLE, WINDOW_STYLE,
        WNDCLASSW, WM_APP, WM_COMMAND, WM_CONTEXTMENU, WM_DESTROY, WM_LBUTTONDBLCLK, WM_LBUTTONUP,
        WM_RBUTTONUP,
    };

    /// Своё сообщение иконки: сюда приходят клики и меню.
    const WM_TRAY: u32 = WM_APP + 1;
    const ID_SHOW: usize = 1;
    const ID_QUIT: usize = 2;
    const TRAY_UID: u32 = 1;

    static SHOW_REQ: AtomicBool = AtomicBool::new(false);
    static QUIT_REQ: AtomicBool = AtomicBool::new(false);
    static ICON_HWND: AtomicIsize = AtomicIsize::new(0);
    /// HWND главного окна. Нужен, чтобы показать его из потока трея: пока окно
    /// спрятано, eframe не гарантирует вызов `update`, и опрос флага из GUI
    /// не срабатывает — команда из меню ждала бы следующего события.
    static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);

    /// Просили показать окно (двойной клик по иконке или пункт меню).
    pub fn take_show() -> bool {
        SHOW_REQ.swap(false, Ordering::Relaxed)
    }

    /// Просили выйти из приложения.
    pub fn take_quit() -> bool {
        QUIT_REQ.swap(false, Ordering::Relaxed)
    }

    /// GUI сообщает HWND главного окна — трей показывает его по команде.
    pub fn set_main_window(hwnd: isize) {
        MAIN_HWND.store(hwnd, Ordering::SeqCst);
    }

    /// Показать главное окно: снять скрытие, поднять и отдать фокус.
    fn reveal_main() {
        let raw = MAIN_HWND.load(Ordering::SeqCst);
        SHOW_REQ.store(true, Ordering::Relaxed);
        if raw == 0 {
            return;
        }
        let main = HWND(raw as *mut _);
        unsafe {
            let _ = ShowWindow(main, SW_SHOW);
            let _ = SetForegroundWindow(main);
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Строки в NOTIFYICONDATAW — фиксированные буферы без длины, копируем с NUL.
    fn put(buf: &mut [u16], text: &str) {
        let w = wide(text);
        let n = w.len().min(buf.len());
        buf[..n].copy_from_slice(&w[..n]);
        for slot in buf.iter_mut().skip(n) {
            *slot = 0;
        }
    }

    fn empty_icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
        NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_UID,
            ..Default::default()
        }
    }

    /// Иконка приложения из собственного exe; если не вышло — системная.
    fn load_icon() -> windows::Win32::UI::WindowsAndMessaging::HICON {
        if let Ok(path) = std::env::current_exe() {
            let w = wide(&path.to_string_lossy());
            let icon = unsafe { ExtractIconW(Some(HINSTANCE::default()), PCWSTR(w.as_ptr()), 0) };
            if !icon.is_invalid() {
                return icon;
            }
        }
        unsafe { LoadIconW(None, IDI_APPLICATION).unwrap_or_default() }
    }

    fn show_menu(hwnd: HWND) {
        unsafe {
            let Ok(menu) = CreatePopupMenu() else {
                return;
            };
            let open = wide("Открыть Albus");
            let quit = wide("Выйти");
            let _ = AppendMenuW(menu, MF_STRING, ID_SHOW, PCWSTR(open.as_ptr()));
            let _ = AppendMenuW(menu, MF_STRING, ID_QUIT, PCWSTR(quit.as_ptr()));
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            // Без SetForegroundWindow меню не закрывается по клику мимо него.
            let _ = SetForegroundWindow(hwnd);
            let _ = TrackPopupMenu(
                menu,
                TPM_RIGHTBUTTON | TPM_BOTTOMALIGN,
                pt.x,
                pt.y,
                None,
                hwnd,
                None,
            );
            let _ = DestroyMenu(menu);
            let _ = PostMessageW(Some(hwnd), 0, WPARAM(0), LPARAM(0));
        }
    }

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_TRAY => {
                match lparam.0 as u32 {
                    WM_LBUTTONDBLCLK | WM_LBUTTONUP => reveal_main(),
                    WM_RBUTTONUP | WM_CONTEXTMENU => show_menu(hwnd),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_COMMAND => {
                match wparam.0 & 0xFFFF {
                    ID_SHOW => reveal_main(),
                    ID_QUIT => {
                        QUIT_REQ.store(true, Ordering::Relaxed);
                        // Закрываем окно сами: спрятанное окно может не получить
                        // ни одного кадра, и опрос флага из GUI не случится.
                        let raw = MAIN_HWND.load(Ordering::SeqCst);
                        if raw != 0 {
                            let main = HWND(raw as *mut _);
                            unsafe {
                                let _ = PostMessageW(Some(main), WM_CLOSE, WPARAM(0), LPARAM(0));
                            }
                        }
                    }
                    _ => {}
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    /// Поднимает поток трея. Идемпотентно: второй вызов ничего не делает.
    pub fn init() {
        static SPAWNED: AtomicBool = AtomicBool::new(false);
        if SPAWNED.swap(true, Ordering::SeqCst) {
            return;
        }
        std::thread::Builder::new()
            .name("albus-tray".into())
            .spawn(|| unsafe {
                let hinst = GetModuleHandleW(None)
                    .map(|h| HINSTANCE(h.0))
                    .unwrap_or_default();
                let class_name = wide("AlbusTrayWindow");
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(wndproc),
                    hInstance: hinst,
                    lpszClassName: PCWSTR(class_name.as_ptr()),
                    ..Default::default()
                };
                if RegisterClassW(&wc) == 0 {
                    eprintln!("[ТРЕЙ] не удалось зарегистрировать класс окна");
                    return;
                }
                let title = wide("Albus");
                let hwnd = match CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    PCWSTR(class_name.as_ptr()),
                    PCWSTR(title.as_ptr()),
                    WINDOW_STYLE(0),
                    0,
                    0,
                    0,
                    0,
                    None,
                    None,
                    Some(hinst),
                    None,
                ) {
                    Ok(h) => h,
                    Err(e) => {
                        eprintln!("[ТРЕЙ] не удалось создать окно: {e}");
                        return;
                    }
                };

                let mut nid = empty_icon_data(hwnd);
                nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
                nid.uCallbackMessage = WM_TRAY;
                nid.hIcon = load_icon();
                put(&mut nid.szTip, "Albus Audio — эфир");
                if !Shell_NotifyIconW(NIM_ADD, &nid).as_bool() {
                    eprintln!("[ТРЕЙ] оболочка не приняла иконку");
                }
                ICON_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
                println!("[ТРЕЙ] иконка в трее, окно {hwnd:?}");

                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }

                let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
                ICON_HWND.store(0, Ordering::SeqCst);
            })
            .ok();
    }

    /// Всплывающее уведомление Windows. Можно звать из GUI-потока.
    pub fn notify(title: &str, text: &str) {
        let raw = ICON_HWND.load(Ordering::SeqCst);
        if raw == 0 {
            return;
        }
        let hwnd = HWND(raw as *mut _);
        let mut nid = empty_icon_data(hwnd);
        nid.uFlags = NIF_INFO;
        nid.dwInfoFlags = NIIF_INFO;
        put(&mut nid.szInfoTitle, title);
        put(&mut nid.szInfo, text);
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn init() {}
    pub fn notify(_title: &str, _text: &str) {}
    pub fn take_show() -> bool {
        false
    }
    pub fn take_quit() -> bool {
        false
    }
    pub fn set_main_window(_hwnd: isize) {}
}

pub use imp::{init, notify, set_main_window, take_quit, take_show};
