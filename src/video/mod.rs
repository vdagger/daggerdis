//! Видеотрансляция.
//!
//! ОС-независимый слой: пресеты, контракт закодированного кадра и общие типы.
//! Захват и аппаратный кодек живут в платформенных модулях (сейчас `win_mf`
//! на Media Foundation; позже ScreenCaptureKit/VideoToolbox на macOS и
//! MediaProjection/MediaCodec на Android).
//!
//! Пайплайн один для экрана и камеры:
//!   источник кадров (BGRA или NV12) → масштаб и NV12 на GPU → H.264 →
//!   iroh-стрим → H.264 → NV12 → RGB на GPU → egui-текстура
//!
//! Источник — это просто поставщик кадров, поэтому камера подключается
//! без изменений в кодеке и транспорте.

#[cfg(windows)]
pub mod publisher;
#[cfg(windows)]
pub mod win_d3d;
#[cfg(windows)]
pub mod win_decode;
#[cfg(windows)]
pub mod win_dxgi;
#[cfg(windows)]
pub mod win_mf;
#[cfg(windows)]
pub mod win_wgc;
#[cfg(windows)]
pub mod win_windows;

/// Пресет качества.
///
/// Битрейт считается от числа пикселей в секунду, а не по таблице
/// разрешений: 1080p, 1440p и 4K идут одним путём.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum VideoPreset {
    Low,
    Balanced,
    Quality,
}

impl VideoPreset {
    pub fn label(self) -> &'static str {
        match self {
            VideoPreset::Low => "Экономный",
            VideoPreset::Balanced => "Сбалансированный",
            VideoPreset::Quality => "Качество",
        }
    }

    /// Ключ для `config.ini`: настройка переживает перезапуск.
    pub fn key(self) -> &'static str {
        match self {
            VideoPreset::Low => "low",
            VideoPreset::Balanced => "balanced",
            VideoPreset::Quality => "quality",
        }
    }

    /// Обратный разбор ключа. Неизвестное значение — не пресет: молча
    /// подставлять «сбалансированный» нельзя, иначе испорченный файл
    /// незаметно меняет качество трансляции.
    pub fn from_key(key: &str) -> Option<Self> {
        match key.trim().to_lowercase().as_str() {
            "low" => Some(VideoPreset::Low),
            "balanced" => Some(VideoPreset::Balanced),
            "quality" => Some(VideoPreset::Quality),
            _ => None,
        }
    }

    /// Все пресеты для выпадающего списка в карточке трансляции.
    pub fn all() -> [VideoPreset; 3] {
        [VideoPreset::Low, VideoPreset::Balanced, VideoPreset::Quality]
    }

    /// Биты на пиксель кадра при H.264 и CBR.
    /// Рабочий стол и текст требуют больше, чем видео с камеры: при 0.10
    /// 1920x1080x60 даёт ~12 Мбит/с, 2560x1440x60 — ~22 Мбит/с, а окно
    /// 1108x610x30 — ~2 Мбит/с (для читаемого текста этого мало, поэтому внизу
    /// есть пол по битрейту). Quality 0.16: 1080p60 ≈ 20 Мбит/с, окно ≈ 3.2.
    pub fn bits_per_pixel(self) -> f64 {
        match self {
            VideoPreset::Low => 0.05,
            VideoPreset::Balanced => 0.10,
            VideoPreset::Quality => 0.16,
        }
    }

    /// Ориентир для карточки трансляции: сколько Мбит/с даёт пресет на
    /// 1080p60. Реальный битрейт считается от размера источника.
    pub fn mbit_1080p60(self) -> f32 {
        (1920.0 * 1080.0 * 60.0 * self.bits_per_pixel() / 1e6) as f32
    }

    /// Разумный пресет по умолчанию для такого разрешения.
    pub fn default_for(width: u32, height: u32) -> Self {
        if width.saturating_mul(height) >= 2560 * 1440 {
            VideoPreset::Quality
        } else {
            VideoPreset::Balanced
        }
    }
}

/// Параметры одного потока.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VideoTarget {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u32,
    /// Интервал опорных кадров, секунды.
    pub keyframe_secs: u32,
}

impl VideoTarget {
    /// Пол по битрейту: ниже него H.264 «мылит» текст в кашу, и никакой пресет
    /// этого не исправит. Окно 1108x610x30 по формуле (0.10) дало бы 2.0 Мбит/с,
    /// а на 1.5 Мбит/с (прежний пол) браузер и Discord выглядели мутно.
    const MIN_BITRATE: u32 = 2_500_000;
    /// До этого битрейта ключевой кадр выдаётся раз в 2 с: на малом потоке
    /// IDR каждую секунду съедает заметную долю битрейта и мылит картинку.
    const SMALL_STREAM_BPS: u32 = 4_000_000;

    pub fn new(width: u32, height: u32, fps: u32, preset: VideoPreset) -> Self {
        // NV12 требует чётных размеров, H.264 — тоже
        let width = (width & !1).max(2);
        let height = (height & !1).max(2);
        let fps = fps.clamp(1, 120);
        let raw = (width as f64) * (height as f64) * (fps as f64) * preset.bits_per_pixel();
        let bitrate_bps = (raw as u32).clamp(Self::MIN_BITRATE, 40_000_000);
        Self {
            width,
            height,
            fps,
            bitrate_bps,
            // Ключевой кадр: 2 с на малом потоке, 1 с на большом. Зритель,
            // вошедший в середину, всё равно получает свежий IDR по подписке
            // (`force_keyframe`), так что плановый интервал можно удлинить.
            keyframe_secs: if bitrate_bps < Self::SMALL_STREAM_BPS {
                2
            } else {
                1
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_bitrates_scale_with_pixels_and_fps() {
        // 1080p60 — опорные значения, по ним карточка показывает ориентир.
        let hd = VideoTarget::new(1920, 1080, 60, VideoPreset::Balanced);
        assert_eq!(hd.bitrate_bps, 12_441_600); // 2.07 Мпикс · 60 · 0.10
        let quality = VideoTarget::new(1920, 1080, 60, VideoPreset::Quality);
        assert_eq!(quality.bitrate_bps, 19_906_560);
        // Окно 1108x610x30 «сбалансированным» — 2.0 Мбит/с по формуле, но пол
        // 2.5 Мбит/с: на 1.5 Мбит/с текст браузера/Discord был мутным.
        let window = VideoTarget::new(1108, 610, 30, VideoPreset::Balanced);
        assert_eq!(window.bitrate_bps, VideoTarget::MIN_BITRATE);
        let window_quality = VideoTarget::new(1108, 610, 30, VideoPreset::Quality);
        assert_eq!(window_quality.bitrate_bps, 3_244_224);
    }

    #[test]
    fn small_streams_get_longer_gop() {
        // Малый поток: IDR раз в 2 с, иначе ключевые кадры съедают битрейт.
        let window = VideoTarget::new(1108, 610, 30, VideoPreset::Balanced);
        assert_eq!(window.keyframe_secs, 2);
        let big = VideoTarget::new(1920, 1080, 60, VideoPreset::Quality);
        assert_eq!(big.keyframe_secs, 1);
    }

    #[test]
    fn odd_sizes_and_fps_are_normalized() {
        let t = VideoTarget::new(1109, 611, 0, VideoPreset::Low);
        assert_eq!((t.width, t.height), (1108, 610));
        assert_eq!(t.fps, 1);
    }
}

/// Описание потока: нужно и превью, и зрителю по сети.
#[derive(Clone, Debug)]
pub struct StreamInfo {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// SPS/PPS — декодер должен получить их до первого кадра.
    pub sequence_header: Vec<u8>,
}

/// Один закодированный кадр, готовый к отправке.
#[derive(Clone, Debug)]
pub struct EncodedUnit {
    pub data: Vec<u8>,
    pub keyframe: bool,
    /// Метка времени в 100-нс тиках (единица Media Foundation).
    pub pts_100ns: i64,
}

#[cfg(not(windows))]
pub fn platform_supported() -> bool {
    false
}

#[cfg(windows)]
pub fn platform_supported() -> bool {
    true
}
