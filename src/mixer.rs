use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

/// Преобразует значение ползунка 0..=300 в логарифмический коэффициент усиления
fn slider_to_gain(val: u32) -> f32 {
    let v = val as f32;
    if v <= 100.0 {
        // 0%..100% -> плавное ослабление от 0.0 до номинала 1.0
        v / 100.0
    } else {
        // 100%..300% -> честный децибельный буст до +18.0 дБ (усиление до 8 раз!)
        let boost_ratio = (v - 100.0) / 200.0; // 0.0 .. 1.0
        let db = boost_ratio * 18.0; // 0.0 .. +18.0 dB
        10.0f32.powf(db / 20.0)
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Свежие данные о голосе собеседника: когда последний раз звучал и насколько громко
#[derive(Debug, Default, Clone, Copy)]
pub struct PeerMeter {
    pub last_audio_ms: u64,
    pub peak: f32,
}

/// Аудио-микшер для управления громкостью и мьютом
#[derive(Debug)]
pub struct AudioMixer {
    mic_muted: AtomicBool,
    ch1_volume: AtomicU32, // Микрофон (Левый)
    ch2_volume: AtomicU32, // Гитара (Правый)
    input1_channel: AtomicU32,
    input2_channel: AtomicU32,
    output_volume: AtomicU32,
    input1_peak: AtomicU32,
    input2_peak: AtomicU32,
    peer_volumes: Mutex<HashMap<u64, u32>>,
    peer_meters: Mutex<HashMap<u64, PeerMeter>>,
}

impl AudioMixer {
    /// Создает новый аудио-микшер с дефолтными значениями
    pub fn new() -> Self {
        Self {
            mic_muted: AtomicBool::new(false),
            ch1_volume: AtomicU32::new(100),
            ch2_volume: AtomicU32::new(100),
            input1_channel: AtomicU32::new(0),
            input2_channel: AtomicU32::new(1),
            output_volume: AtomicU32::new(100),
            input1_peak: AtomicU32::new(0),
            input2_peak: AtomicU32::new(0),
            peer_volumes: Mutex::new(HashMap::new()),
            peer_meters: Mutex::new(HashMap::new()),
        }
    }

    /// Применяет общий мьют к сведённому входному буферу
    /// (поканальное усиление и VU-пики считаются в аудио-колбэке)
    pub fn process_input(&self, buffer: &mut [f32]) {
        if self.mic_muted.load(Ordering::Relaxed) {
            buffer.fill(0.0);
        }
    }

    /// Обрабатывает выходной аудио-буфер с логарифмическим бустом и студийным лимитером (-1 dBFS)
    pub fn process_output(&self, peer_id: u64, buffer: &mut [f32]) {
        let volume_gain = slider_to_gain(self.output_volume.load(Ordering::Relaxed));
        let peer_gain = slider_to_gain(self.get_peer_volume(peer_id));
        let combined_gain = volume_gain * peer_gain;

        // Порог лимитера: -1.0 dBFS = 0.891
        const THRESHOLD: f32 = 0.891;

        let mut peak = 0.0f32;
        for sample in buffer.iter_mut() {
            let boosted = *sample * combined_gain;
            let abs_val = boosted.abs();

            if abs_val <= THRESHOLD {
                // В рабочем диапазоне звук остается 100% прозрачным, без искажений
                *sample = boosted;
            } else {
                // Мягкое компрессионное колено: пики сглаживаются до 0.99 без треска и клиппинга
                let excess = abs_val - THRESHOLD;
                let compressed = THRESHOLD + (1.0 - THRESHOLD) * (excess / (1.0 + excess));
                *sample = boosted.signum() * compressed.min(0.99);
            }
            peak = peak.max(sample.abs());
        }

        // Отмечаем, когда собеседник реально звучал: тишина в пакетах не считается речью
        if let Ok(mut meters) = self.peer_meters.lock() {
            let meter = meters.entry(peer_id).or_default();
            if peak > 0.02 {
                meter.last_audio_ms = now_ms();
                meter.peak = peak.min(1.0);
            } else {
                meter.peak *= 0.35;
            }
        }
    }

    /// Пиковый уровень собеседника для мини-индикатора (0..=1)
    pub fn peer_meter(&self, peer_id: u64) -> PeerMeter {
        self.peer_meters
            .lock()
            .ok()
            .and_then(|m| m.get(&peer_id).copied())
            .unwrap_or_default()
    }

    /// Говорит ли собеседник прямо сейчас (звук был меньше 400 мс назад)
    pub fn is_peer_speaking(&self, peer_id: u64) -> bool {
        let meter = self.peer_meter(peer_id);
        meter.last_audio_ms > 0 && now_ms().saturating_sub(meter.last_audio_ms) < 400
    }

    /// Устанавливает громкость конкретного участника (0-300)
    pub fn set_peer_volume(&self, peer_id: u64, volume: u32) {
        let mut volumes = self.peer_volumes.lock().unwrap();
        volumes.insert(peer_id, volume.min(300));
    }

    /// Получает громкость конкретного участника (по умолчанию 100)
    pub fn get_peer_volume(&self, peer_id: u64) -> u32 {
        let volumes = self.peer_volumes.lock().unwrap();
        volumes.get(&peer_id).copied().unwrap_or(100)
    }

    pub fn is_mic_muted(&self) -> bool {
        self.mic_muted.load(Ordering::Relaxed)
    }

    pub fn set_mic_muted(&self, muted: bool) {
        self.mic_muted.store(muted, Ordering::Relaxed);
    }

    pub fn ch1_volume(&self) -> u32 {
        self.ch1_volume.load(Ordering::Relaxed)
    }

    pub fn set_ch1_volume(&self, volume: u32) {
        self.ch1_volume.store(volume.min(100), Ordering::Relaxed);
    }

    pub fn ch2_volume(&self) -> u32 {
        self.ch2_volume.load(Ordering::Relaxed)
    }

    pub fn set_ch2_volume(&self, volume: u32) {
        self.ch2_volume.store(volume.min(100), Ordering::Relaxed);
    }

    /// Канал устройства для Входа 1 (u32::MAX = выключен)
    pub fn input1_channel(&self) -> u32 {
        self.input1_channel.load(Ordering::Relaxed)
    }

    pub fn set_input1_channel(&self, channel: u32) {
        self.input1_channel.store(channel, Ordering::Relaxed);
    }

    /// Канал устройства для Входа 2 (u32::MAX = выключен)
    pub fn input2_channel(&self) -> u32 {
        self.input2_channel.load(Ordering::Relaxed)
    }

    pub fn set_input2_channel(&self, channel: u32) {
        self.input2_channel.store(channel, Ordering::Relaxed);
    }

    pub fn output_volume(&self) -> u32 {
        self.output_volume.load(Ordering::Relaxed)
    }

    pub fn set_output_volume(&self, volume: u32) {
        self.output_volume.store(volume.min(300), Ordering::Relaxed);
    }

    /// Пиковый уровень Входа 1 (0..=1) для VU-метра
    pub fn input1_peak(&self) -> f32 {
        self.input1_peak.load(Ordering::Relaxed) as f32 / 100.0
    }

    pub fn set_input1_peak_raw(&self, peak: u32) {
        self.input1_peak.store(peak, Ordering::Relaxed);
    }

    /// Пиковый уровень Входа 2 (0..=1) для VU-метра
    pub fn input2_peak(&self) -> f32 {
        self.input2_peak.load(Ordering::Relaxed) as f32 / 100.0
    }

    pub fn set_input2_peak_raw(&self, peak: u32) {
        self.input2_peak.store(peak, Ordering::Relaxed);
    }
}

impl Default for AudioMixer {
    fn default() -> Self {
        Self::new()
    }
}
