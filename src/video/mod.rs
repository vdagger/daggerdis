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

    /// Биты на пиксель кадра при H.264 и длинном GOP.
    /// Рабочий стол и текст требуют больше, чем видео: 0.10 при 1920x1080x60
    /// даёт ~12 Мбит/с, при 2560x1440x60 — ~22 Мбит/с.
    pub fn bits_per_pixel(self) -> f64 {
        match self {
            VideoPreset::Low => 0.04,
            VideoPreset::Balanced => 0.07,
            VideoPreset::Quality => 0.10,
        }
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
    pub fn new(width: u32, height: u32, fps: u32, preset: VideoPreset) -> Self {
        // NV12 требует чётных размеров, H.264 — тоже
        let width = (width & !1).max(2);
        let height = (height & !1).max(2);
        let fps = fps.clamp(1, 120);
        let raw = (width as f64) * (height as f64) * (fps as f64) * preset.bits_per_pixel();
        Self {
            width,
            height,
            fps,
            bitrate_bps: (raw as u32).clamp(1_500_000, 40_000_000),
            keyframe_secs: 2,
        }
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
