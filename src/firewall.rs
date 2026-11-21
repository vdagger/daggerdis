//! Модуль автоматической настройки сетевого экрана Windows

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Имя правила. Историческое — старые копии приложения уже наплодили
/// правил с этим именем, и чистить их надо по нему же.
#[cfg(windows)]
const RULE_NAME: &str = "DiscordRF";

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

#[cfg(windows)]
fn netsh(args: &[&str]) -> std::io::Result<std::process::Output> {
    std::process::Command::new("netsh")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
}

/// Есть ли уже правило для именно этого exe. Netsh плодил новое правило на
/// каждый запуск: `add rule` с тем же именем не ошибка, поэтому в брандмауэре
/// копились десятки одинаковых записей. Сравниваем по полному пути программы —
/// он печатается в `show rule` как есть, независимо от языка системы.
/// Код возврата netsh здесь не помогает: он равен 0 и когда правила нет
/// («No rules match the specified criteria» тоже успех).
#[cfg(windows)]
fn rule_exists(exe_str: &str) -> bool {
    let out = match netsh(&[
        "advfirewall",
        "firewall",
        "show",
        "rule",
        &format!("name={}", RULE_NAME),
        "verbose",
    ]) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[ФАЕРВОЛ] Не удалось прочитать правила: {}", e);
            return false;
        }
    };
    // Вывод netsh — в OEM-кодировке, но путь программы в нём ASCII,
    // поэтому поиск подстроки переживает перекодировку.
    let text = String::from_utf8_lossy(&out.stdout);
    text.contains(exe_str)
}

/// Удаляет все правила с нашим именем (в том числе от старых копий exe),
/// чтобы вместо них осталось ровно одно актуальное.
#[cfg(windows)]
fn remove_all_rules() {
    let _ = netsh(&[
        "advfirewall",
        "firewall",
        "delete",
        "rule",
        &format!("name={}", RULE_NAME),
    ]);
}

/// Добавляет правило в Брандмауэр Windows для разрешения входящих UDP-пакетов.
/// Повторный запуск ничего не добавляет, если правило для этого exe уже есть.
pub fn setup_firewall_rules() {
    // На Linux/macOS правило входящего UDP не нужно: iroh поднимает исходящий
    // QUIC, а проход через NAT делает релей (или hole punching с уже открытого
    // сокета). Если понадобится — это `ufw allow` / `pfctl`, но по умолчанию
    // приложение работает без прав администратора.
    #[cfg(not(windows))]
    println!("[ФАЕРВОЛ] Не Windows: правило входящего UDP не требуется (iroh + NAT/релей).");

    #[cfg(windows)]
    {
        let Ok(exe_path) = std::env::current_exe() else {
            return;
        };
        let exe_str = exe_path.to_string_lossy().into_owned();

        println!(
            "[ФАЕРВОЛ] Проверка и настройка Брандмауэра Windows для: {}",
            exe_str
        );

        if rule_exists(&exe_str) {
            println!("[ФАЕРВОЛ] Правило уже есть — новых записей не создаём.");
            return;
        }

        // Правил от других копий exe (другой путь) может быть много —
        // убираем их, прежде чем добавить одно актуальное.
        remove_all_rules();

        let output = netsh(&[
            "advfirewall",
            "firewall",
            "add",
            "rule",
            &format!("name={}", RULE_NAME),
            "dir=in",
            "action=allow",
            &format!("program={}", exe_str),
            "enable=yes",
            "profile=any",
            "protocol=UDP",
        ]);

        match output {
            Ok(out) => {
                if out.status.success() {
                    println!("[ФАЕРВОЛ] Входящий UDP успешно разрешен в Брандмауэре Windows.");
                } else {
                    println!("[ФАЕРВОЛ] Заметка: Для автоматического добавления правила фаервола может потребоваться запуск от Администратора.");
                }
            }
            Err(e) => {
                eprintln!(
                    "[ФАЕРВОЛ] Не удалось выполнить команду настройки фаервола: {}",
                    e
                );
            }
        }
    }
}
