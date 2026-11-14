//! Персистентные настройки приложения: %APPDATA%\Albus\config.ini
//!
//! Формат — простой `ключ=значение` по строке: своего парсера достаточно,
//! и он не тянет новых зависимостей. Устройства хранятся по имени: индексы
//! cpal меняются при подключении новых устройств, а имя остаётся.

use std::path::PathBuf;

#[derive(Debug, Clone, Default)]
pub struct AppConfig {
    pub input_device: Option<String>,
    pub output_device: Option<String>,
    pub ptt_vk: Option<i32>,
    pub ch1_volume: Option<u32>,
    pub ch2_volume: Option<u32>,
    pub output_volume: Option<u32>,
    pub input1_channel: Option<u32>,
    pub input2_channel: Option<u32>,
    pub continuous_mode: Option<bool>,
}

impl AppConfig {
    /// Путь к файлу настроек: %APPDATA%\Albus\config.ini
    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
        Some(base.join("Albus").join("config.ini"))
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
            match key.trim() {
                "input_device" => cfg.input_device = Some(value.to_string()),
                "output_device" => cfg.output_device = Some(value.to_string()),
                "ptt_vk" => cfg.ptt_vk = value.parse().ok(),
                "ch1_volume" => cfg.ch1_volume = value.parse().ok(),
                "ch2_volume" => cfg.ch2_volume = value.parse().ok(),
                "output_volume" => cfg.output_volume = value.parse().ok(),
                "input1_channel" => cfg.input1_channel = value.parse().ok(),
                "input2_channel" => cfg.input2_channel = value.parse().ok(),
                "continuous_mode" => cfg.continuous_mode = value.parse().ok(),
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
        if let Some(value) = self.continuous_mode {
            out.push_str(&format!("continuous_mode={}\n", value));
        }
        out
    }
}
