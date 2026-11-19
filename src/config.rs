//! Персистентные настройки приложения: %APPDATA%\Albus\config.ini
//!
//! Формат — простой `ключ=значение` по строке: своего парсера достаточно,
//! и он не тянет новых зависимостей. Устройства хранятся по имени: индексы
//! cpal меняются при подключении новых устройств, а имя остаётся.

use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    pub input_device: Option<String>,
    /// Второй физический вход. Пусто или совпадает с `input_device` — входы
    /// берутся с двух каналов одной карты, как раньше.
    pub input2_device: Option<String>,
    pub output_device: Option<String>,
    pub ptt_vk: Option<i32>,
    pub ch1_volume: Option<u32>,
    pub ch2_volume: Option<u32>,
    pub output_volume: Option<u32>,
    pub input1_channel: Option<u32>,
    pub input2_channel: Option<u32>,
    /// Профиль обработки входа: 0 — голос, 1 — инструмент.
    pub input1_profile: Option<u32>,
    pub input2_profile: Option<u32>,
    pub continuous_mode: Option<bool>,
    /// Частота трансляции экрана (30 или 60 fps); по умолчанию 60.
    pub video_fps: Option<u32>,
    pub nick: Option<String>,
    /// Гейт + AGC + лимитер на микрофоне. По умолчанию выключено.
    pub mic_processing: Option<bool>,
    /// Лимитер на общем миксе собеседников. По умолчанию выключено.
    pub rx_limiter: Option<bool>,
    pub peer_volumes: HashMap<u64, u32>,
}

/// Каталог данных приложения: %APPDATA%\Albus (или ~/.config/Albus вне Windows).
pub fn albus_dir() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(base.join("Albus"))
}

impl AppConfig {
    /// Путь к файлу настроек: %APPDATA%\Albus\config.ini
    pub fn path() -> Option<PathBuf> {
        albus_dir().map(|d| d.join("config.ini"))
    }

    /// Читает настройки с диска; любые ошибки трактуются как «настроек нет»
    pub fn load() -> Self {
        let Some(path) = Self::path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text),
            Err(_) => Self::default(),
        }
    }

    /// Сохраняет настройки на диск, создавая каталог при необходимости
    pub fn save(&self) {
        let Some(path) = Self::path() else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&path, self.to_text()) {
            eprintln!("[КОНФИГ] Не удалось сохранить настройки: {}", e);
        }
    }

    fn parse(text: &str) -> Self {
        let mut cfg = Self::default();
        for line in text.lines() {
            let line = line.trim_end_matches('\r').trim_start();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            // Per-peer громкость: peer_<id>_volume=<0..300>. id — стабильный псевдоним
            // NodeID (my_id), поэтому громкость переживает перезапуск.
            if let Some(id_str) = key.strip_prefix("peer_").and_then(|s| s.strip_suffix("_volume")) {
                if let (Ok(id), Ok(vol)) = (id_str.parse::<u64>(), value.parse::<u32>()) {
                    cfg.peer_volumes.insert(id, vol);
                }
                continue;
            }
            match key {
                "input_device" => cfg.input_device = Some(value.to_string()),
                // Пустое значение — «второй вход с той же карты».
                "input2_device" if !value.is_empty() => {
                    cfg.input2_device = Some(value.to_string())
                }
                "output_device" => cfg.output_device = Some(value.to_string()),
                "ptt_vk" => cfg.ptt_vk = value.parse().ok(),
                "ch1_volume" => cfg.ch1_volume = value.parse().ok(),
                "ch2_volume" => cfg.ch2_volume = value.parse().ok(),
                "output_volume" => cfg.output_volume = value.parse().ok(),
                "input1_channel" => cfg.input1_channel = value.parse().ok(),
                "input2_channel" => cfg.input2_channel = value.parse().ok(),
                // Профили: всё, кроме 0 и 1, отбрасываем — неизвестный профиль
                // молча стал бы третьим режимом обработки.
                "input1_profile" => cfg.input1_profile = value.parse().ok().filter(|v| *v <= 1),
                "input2_profile" => cfg.input2_profile = value.parse().ok().filter(|v| *v <= 1),
                "continuous_mode" => cfg.continuous_mode = value.parse().ok(),
                "video_fps" => cfg.video_fps = value.parse().ok(),
                "nick" => cfg.nick = Some(value.to_string()),
                "mic_processing" => cfg.mic_processing = value.parse().ok(),
                "rx_limiter" => cfg.rx_limiter = value.parse().ok(),
                _ => {}
            }
        }
        cfg
    }

    fn to_text(&self) -> String {
        let mut out = String::new();
        if let Some(value) = &self.input_device {
            out.push_str(&format!("input_device={}\n", value));
        }
        if let Some(value) = &self.input2_device {
            out.push_str(&format!("input2_device={}\n", value));
        }
        if let Some(value) = &self.output_device {
            out.push_str(&format!("output_device={}\n", value));
        }
        if let Some(value) = self.ptt_vk {
            out.push_str(&format!("ptt_vk={}\n", value));
        }
        if let Some(value) = self.ch1_volume {
            out.push_str(&format!("ch1_volume={}\n", value));
        }
        if let Some(value) = self.ch2_volume {
            out.push_str(&format!("ch2_volume={}\n", value));
        }
        if let Some(value) = self.output_volume {
            out.push_str(&format!("output_volume={}\n", value));
        }
        if let Some(value) = self.input1_channel {
            out.push_str(&format!("input1_channel={}\n", value));
        }
        if let Some(value) = self.input2_channel {
            out.push_str(&format!("input2_channel={}\n", value));
        }
        if let Some(value) = self.input1_profile {
            out.push_str(&format!("input1_profile={}\n", value));
        }
        if let Some(value) = self.input2_profile {
            out.push_str(&format!("input2_profile={}\n", value));
        }
        if let Some(value) = self.continuous_mode {
            out.push_str(&format!("continuous_mode={}\n", value));
        }
        if let Some(value) = self.video_fps {
            out.push_str(&format!("video_fps={}\n", value));
        }
        if let Some(value) = &self.nick {
            out.push_str(&format!("nick={}\n", value));
        }
        if let Some(value) = self.mic_processing {
            out.push_str(&format!("mic_processing={}\n", value));
        }
        if let Some(value) = self.rx_limiter {
            out.push_str(&format!("rx_limiter={}\n", value));
        }
        let mut vols: Vec<(u64, u32)> = self.peer_volumes.iter().map(|(k, v)| (*k, *v)).collect();
        vols.sort();
        for (id, vol) in vols {
            out.push_str(&format!("peer_{}_volume={}\n", id, vol));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_config_has_no_second_input() {
        // Конфиг до появления второго входа должен читаться без сюрпризов.
        let cfg = AppConfig::parse("input_device=Микрофон\ninput2_channel=1\nch1_volume=80\n");
        assert_eq!(cfg.input_device.as_deref(), Some("Микрофон"));
        assert!(cfg.input2_device.is_none());
        assert_eq!(cfg.input1_profile, None);
        assert_eq!(cfg.ch1_volume, Some(80));
    }

    #[test]
    fn new_keys_roundtrip() {
        let cfg = AppConfig::parse(
            "input2_device=Focusrite\ninput1_profile=0\ninput2_profile=1\n",
        );
        assert_eq!(cfg.input2_device.as_deref(), Some("Focusrite"));
        assert_eq!(cfg.input1_profile, Some(0));
        assert_eq!(cfg.input2_profile, Some(1));
        let again = AppConfig::parse(&cfg.to_text());
        assert_eq!(again.input2_device, cfg.input2_device);
        assert_eq!(again.input1_profile, cfg.input1_profile);
        assert_eq!(again.input2_profile, cfg.input2_profile);
    }

    #[test]
    fn empty_second_device_is_none() {
        let cfg = AppConfig::parse("input2_device=\n");
        assert!(cfg.input2_device.is_none());
    }

    #[test]
    fn out_of_range_profile_ignored() {
        let cfg = AppConfig::parse("input1_profile=7\ninput2_profile=voice\n");
        assert_eq!(cfg.input1_profile, None);
        assert_eq!(cfg.input2_profile, None);
    }
}
