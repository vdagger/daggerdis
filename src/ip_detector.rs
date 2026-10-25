//! Модуль для определения внешнего IP-адреса пользователя.

use std::time::Duration;

/// Получает внешний IP-адрес пользователя через публичный сервис.
/// Возвращает IP-адрес в виде строки или сообщение об ошибке.
pub fn get_external_ip() -> String {
    // Используем ipify.org для получения внешнего IP
    let response = ureq::get("http://api.ipify.org")
        .timeout(Duration::from_secs(5))
        .call();
    
    match response {
        Ok(response) => {
            let ip = response.into_string().unwrap_or_else(|_| String::from("Не удалось определить IP"));
            ip
        }
        Err(_) => {
            // Пытаемся использовать резервный URL, если первый не работает
            let response = ureq::get("http://icanhazip.com")
                .timeout(Duration::from_secs(5))
                .call();
            
            match response {
                Ok(response) => {
                    let ip = response.into_string().unwrap_or_else(|_| String::from("Не удалось определить IP"));
                    ip
                }
                Err(_) => {
                    // Пытаемся использовать третий резервный URL
                    let response = ureq::get("http://ident.me")
                        .timeout(Duration::from_secs(5))
                        .call();
                    
                    match response {
                        Ok(response) => {
                            let ip = response.into_string().unwrap_or_else(|_| String::from("Не удалось определить IP"));
                            ip
                        }
                        Err(_) => String::from("Не удалось определить IP"),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_external_ip_returns_string() {
        // Тест на возврат строки (даже если не удастся получить IP)
        let result = get_external_ip();
        assert!(result.len() > 0);
    }
}