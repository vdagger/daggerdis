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
    /// Профиль обработки входа: 0 — голос, 1 — инструмент. Читается только
    /// из старых конфигов: профиль теперь выводится из тумблеров ниже.
    pub input1_profile: Option<u32>,
    pub input2_profile: Option<u32>,
    /// Шумодав RNNoise на входе (отдельный от затвора и AGC).
    pub input1_denoise: Option<bool>,
    pub input2_denoise: Option<bool>,
    /// Эхо-подавление AEC3 на входе — независимо от профилей.
    pub input1_aec: Option<bool>,
    pub input2_aec: Option<bool>,
    /// Эффекты каждого входа: затвор и AGC включаются независимо.
    pub input1_gate: Option<bool>,
    pub input1_agc: Option<bool>,
    pub input2_gate: Option<bool>,
    pub input2_agc: Option<bool>,
    pub continuous_mode: Option<bool>,
    /// Частота трансляции экрана (30 или 60 fps); по умолчанию 60.
    pub video_fps: Option<u32>,
    pub nick: Option<String>,
    /// Старый общий тумблер «обработка звука» (гейт + AGC + лимитер).
    /// Читается, только когда новых ключей эффектов в файле нет.
    pub mic_processing: Option<bool>,
    /// Лимитер на передаваемом миксе. По умолчанию выключено.
    pub tx_limiter: Option<bool>,
    /// Лимитер на общем миксе собеседников. По умолчанию выключено.
    pub rx_limiter: Option<bool>,
    /// Битрейт Opus на передачу (64 000 / 96 000 / 128 000), по умолчанию 64k.
    pub opus_bitrate: Option<u32>,
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
                "input1_denoise" => cfg.input1_denoise = value.parse().ok(),
                "input2_denoise" => cfg.input2_denoise = value.parse().ok(),
                "input1_aec" => cfg.input1_aec = value.parse().ok(),
                "input2_aec" => cfg.input2_aec = value.parse().ok(),
                "input1_gate" => cfg.input1_gate = value.parse().ok(),
                "input1_agc" => cfg.input1_agc = value.parse().ok(),
                "input2_gate" => cfg.input2_gate = value.parse().ok(),
                "input2_agc" => cfg.input2_agc = value.parse().ok(),
                "continuous_mode" => cfg.continuous_mode = value.parse().ok(),
                "video_fps" => cfg.video_fps = value.parse().ok(),
                "nick" => cfg.nick = Some(value.to_string()),
                "mic_processing" => cfg.mic_processing = value.parse().ok(),
                "tx_limiter" => cfg.tx_limiter = value.parse().ok(),
                "rx_limiter" => cfg.rx_limiter = value.parse().ok(),
                // Битрейт — только из допустимого набора: промежуточное значение
                // просто округлится при загрузке в микшер, но хранить мусор незачем.
                "opus_bitrate" => cfg.opus_bitrate = value.parse().ok().filter(|v| {
                    crate::mixer::OPUS_BITRATES.contains(v)
                }),
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
        for (key, value) in [
            ("input1_denoise", self.input1_denoise),
            ("input2_denoise", self.input2_denoise),
            ("input1_aec", self.input1_aec),
            ("input2_aec", self.input2_aec),
            ("input1_gate", self.input1_gate),
            ("input1_agc", self.input1_agc),
            ("input2_gate", self.input2_gate),
            ("input2_agc", self.input2_agc),
        ] {
            if let Some(value) = value {
                out.push_str(&format!("{}={}\n", key, value));
            }
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
        if let Some(value) = self.tx_limiter {
            out.push_str(&format!("tx_limiter={}\n", value));
        }
        if let Some(value) = self.rx_limiter {
            out.push_str(&format!("rx_limiter={}\n", value));
        }
        if let Some(value) = self.opus_bitrate {
            out.push_str(&format!("opus_bitrate={}\n", value));
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
    fn opus_bitrate_only_known_values() {
        let cfg = AppConfig::parse("opus_bitrate=96000\n");
        assert_eq!(cfg.opus_bitrate, Some(96_000));
        let odd = AppConfig::parse("opus_bitrate=97000\n");
        assert_eq!(odd.opus_bitrate, None, "промежуточное значение не храним");
        let text = AppConfig::parse("opus_bitrate=128000\n").to_text();
        assert!(text.contains("opus_bitrate=128000"));
    }

    #[test]
    fn out_of_range_profile_ignored() {
        let cfg = AppConfig::parse("input1_profile=7\ninput2_profile=voice\n");
        assert_eq!(cfg.input1_profile, None);
        assert_eq!(cfg.input2_profile, None);
    }

    #[test]
    fn effect_keys_roundtrip() {
        let cfg = AppConfig::parse(
            "input1_gate=true\ninput1_agc=false\ninput2_gate=true\ntx_limiter=true\ninput2_agc=yes\ninput1_denoise=true\ninput2_denoise=false\ninput1_aec=true\ninput2_aec=false\n",
        );
        assert_eq!(cfg.input1_denoise, Some(true));
        assert_eq!(cfg.input2_denoise, Some(false));
        assert_eq!(cfg.input1_aec, Some(true));
        assert_eq!(cfg.input2_aec, Some(false));
        assert_eq!(cfg.input1_gate, Some(true));
        assert_eq!(cfg.input1_agc, Some(false));
        assert_eq!(cfg.input2_gate, Some(true));
        assert_eq!(cfg.tx_limiter, Some(true));
        assert_eq!(cfg.input2_agc, None, "мусор в bool не должен включать эффект");
        let again = AppConfig::parse(&cfg.to_text());
        assert_eq!(again.input1_denoise, cfg.input1_denoise);
        assert_eq!(again.input2_denoise, cfg.input2_denoise);
        assert_eq!(again.input1_aec, cfg.input1_aec);
        assert_eq!(again.input2_aec, cfg.input2_aec);
        assert_eq!(again.input1_gate, cfg.input1_gate);
        assert_eq!(again.input1_agc, cfg.input1_agc);
        assert_eq!(again.input2_gate, cfg.input2_gate);
        assert_eq!(again.tx_limiter, cfg.tx_limiter);
    }

    #[test]
    fn legacy_mic_processing_still_parsed() {
        let cfg = AppConfig::parse("mic_processing=true\ninput1_profile=0\n");
        assert_eq!(cfg.mic_processing, Some(true));
        assert_eq!(cfg.input1_profile, Some(0));
        assert_eq!(cfg.input1_gate, None, "новых ключей нет — старые решают");
    }
}
