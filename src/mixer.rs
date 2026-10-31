use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::collections::HashMap;

/// Преобразует значение ползунка 0..=300 в логарифмический коэффициент усиления
fn slider_to_gain(val: u32) -> f32 {
    let v = val as f32;
    if v <= 100.0 {
        // 0%..100% -> плавное ослабление от 0.0 до номинала 1.0
        v / 100.0
    } else {
        // 100%..300% -> честный децибельный буст до +18.0 дБ (усиление до 8 раз!)
        let boost_ratio = (v - 100.0) / 200.0; // 0.0 .. 1.0
        let db = boost_ratio * 18.0;           // 0.0 .. +18.0 dB
        10.0f32.powf(db / 20.0)
    }
}

/// Аудио-микшер для управления громкостью и мьютом
#[derive(Debug)]
pub struct AudioMixer {
    mic_muted: AtomicBool,
    ch1_volume: AtomicU32, // Микрофон (Левый)
    ch2_volume: AtomicU32, // Гитара (Правый)
    output_volume: AtomicU32,
    input_peak: Arc<std::sync::atomic::AtomicU32>,
    peer_volumes: Mutex<HashMap<u64, u32>>,
}

impl AudioMixer {
    /// Создает новый аудио-микшер с дефолтными значениями
    pub fn new() -> Self {
        Self {
            mic_muted: AtomicBool::new(false),
            ch1_volume: AtomicU32::new(100),
            ch2_volume: AtomicU32::new(100),
            output_volume: AtomicU32::new(100),
            input_peak: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            peer_volumes: Mutex::new(HashMap::new()),
        }
    }

    /// Обрабатывает входной аудио-буфер (свой микрофон)
    pub fn process_input(&self, buffer: &mut [f32]) {
        if self.mic_muted.load(Ordering::Relaxed) {
            buffer.fill(0.0);
        } else {
            let volume = self.ch1_volume.load(Ordering::Relaxed) as f32 / 100.0;
            for sample in buffer.iter_mut() {
                *sample *= volume;
            }
            
            // Обновляем пиковую амплитуду для VU-метра
            let max_sample = buffer.iter().map(|&x| x.abs()).fold(0.0, f32::max);
            let peak = (max_sample * 100.0) as u32;
            self.input_peak.store(peak, Ordering::Relaxed);
        }
    }

    /// Обрабатывает выходной аудио-буфер с логарифмическим бустом и студийным лимитером (-1 dBFS)
    pub fn process_output(&self, peer_id: u64, buffer: &mut [f32]) {
        let volume_gain = slider_to_gain(self.output_volume.load(Ordering::Relaxed));
        let peer_gain = slider_to_gain(self.get_peer_volume(peer_id));
        let combined_gain = volume_gain * peer_gain;
        
        // Порог лимитера: -1.0 dBFS = 0.891
        const THRESHOLD: f32 = 0.891;

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
        }
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

    pub fn output_volume(&self) -> u32 {
        self.output_volume.load(Ordering::Relaxed)
    }

    pub fn set_output_volume(&self, volume: u32) {
        self.output_volume.store(volume.min(300), Ordering::Relaxed);
    }

    pub fn get_input_peak(&self) -> f32 {
        self.input_peak.load(Ordering::Relaxed) as f32 / 100.0
    }
    
    pub fn set_input_peak_raw(&self, peak: u32) {
        self.input_peak.store(peak, Ordering::Relaxed);
    }
}

impl Default for AudioMixer {
    fn default() -> Self {
        Self::new()
    }
}