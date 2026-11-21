//! Автозапуск при входе в Windows — ключ `Run` в HKCU.
//!
//! Через `reg.exe`, а не через Win32-реестр: операция одна и только по клику в
//! настройках, зато не нужен ни `unsafe`, ни отдельные фичи `windows`, и код
//! собирается на любой платформе (там функции просто отвечают «нет»).

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE: &str = "AlbusAudio";

/// Командная строка для ключа Run.
///
/// Путь обязательно в кавычках: без них Windows режет
/// `C:\Program Files\Albus\albus.exe` по первому пробелу и при входе пытается
/// запустить `C:\Program`.
pub fn command_line(path: &str) -> String {
    format!("\"{path}\"")
}

fn exe_path() -> Option<String> {
    std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Включён ли автозапуск именно для текущего exe.
///
/// Сверяем путь, а не только наличие значения: после переустановки или
/// переезда папки в ключе остаётся старая запись, и «галочка стоит», хотя
/// запускается уже другой файл.
pub fn is_enabled() -> bool {
    let Some(exe) = exe_path() else {
        return false;
    };
    let want = command_line(&exe);
    match std::process::Command::new("reg")
        .args(["query", RUN_KEY, "/v", VALUE])
        .output()
    {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            text.contains(&want)
        }
        _ => false,
    }
}

/// Ставит или снимает автозапуск. `false` — не получилось.
pub fn set_enabled(on: bool) -> bool {
    let Some(exe) = exe_path() else {
        return false;
    };
    let status = if on {
        let cmd = command_line(&exe);
        std::process::Command::new("reg")
            .args([
                "add", RUN_KEY, "/v", VALUE, "/t", "REG_SZ", "/d", &cmd, "/f",
            ])
            .status()
    } else {
        std::process::Command::new("reg")
            .args(["delete", RUN_KEY, "/v", VALUE, "/f"])
            .status()
    };
    status.map(|s| s.success()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_path_with_spaces() {
        assert_eq!(
            command_line(r"C:\Program Files\Albus\albus.exe"),
            "\"C:\\Program Files\\Albus\\albus.exe\""
        );
    }

    #[test]
    fn quotes_even_plain_path() {
        // Кавычки всегда: путь без пробелов сегодня — завтра «Program Files».
        assert_eq!(command_line("albus.exe"), "\"albus.exe\"");
    }
}
