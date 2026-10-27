//! Модуль автоматической настройки сетевого экрана Windows

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Добавляет правило в Брандмауэр Windows для разрешения входящих UDP-пакетов
pub fn setup_firewall_rules() {
    #[cfg(windows)]
    {
        if let Ok(exe_path) = std::env::current_exe() {
            let exe_str = exe_path.to_string_lossy();
            const CREATE_NO_WINDOW: u32 = 0x08000000;

            println!("[ФАЕРВОЛ] Проверка и настройка Брандмауэра Windows для: {}", exe_str);

            // Добавляем разрешающее правило для входящего UDP-трафика
            let output = std::process::Command::new("netsh")
                .args(&[
                    "advfirewall", "firewall", "add", "rule",
                    "name=DiscordRF",
                    "dir=in",
                    "action=allow",
                    &format!("program={}", exe_str),
                    "enable=yes",
                    "profile=any",
                    "protocol=UDP",
                ])
                .creation_flags(CREATE_NO_WINDOW)
                .output();

            match output {
                Ok(out) => {
                    if out.status.success() {
                        println!("[ФАЕРВОЛ] Входящий UDP успешно разрешен в Брандмауэре Windows.");
                    } else {
                        println!("[ФАЕРВОЛ] Заметка: Для автоматического добавления правила фаервола может потребоваться запуск от Администратора.");
                    }
                }
                Err(e) => {
                    eprintln!("[ФАЕРВОЛ] Не удалось выполнить команду настройки фаервола: {}", e);
                }
            }
        }
    }
}