use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::collections::HashMap;

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
    /// ch1_volume = 100, ch2_volume = 100, output_volume = 100, muted = false
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
    /// Если микрофон замьючен, заполняет буфер нулями
    /// Иначе умножает каждый сэмпл на уровень громкости
    pub fn process_input(&self, buffer: &mut [f32]) {
        if self.mic_muted.load(Ordering::Relaxed) {
            buffer.fill(0.0);
        } else {
            let volume = self.ch1_volume.load(Ordering::Relaxed) as f32 / 100.0;
            for sample in buffer.iter_mut() {
                *sample *= volume;
            }
            
            // Обновляем пиковую амплитуду
            let max_sample = buffer.iter().map(|&x| x.abs()).fold(0.0, f32::max);
            let peak = (max_sample * 100.0) as u32;
            self.input_peak.store(peak, Ordering::Relaxed);
        }
    }

    /// Обрабатывает выходной аудио-буфер (громкость собеседников)
    /// Умножает каждый сэмпл на уровень громкости с ограничением (-1.0, 1.0)
    pub fn process_output(&self, peer_id: u64, buffer: &mut [f32]) {
        let volume = self.output_volume.load(Ordering::Relaxed) as f32 / 100.0;
        let peer_volume = self.get_peer_volume(peer_id) as f32 / 100.0;
        let combined_volume = volume * peer_volume;
        
        for sample in buffer.iter_mut() {
            *sample *= combined_volume;
            *sample = sample.clamp(-1.0, 1.0);
        }
    }

    /// Устанавливает громкость конкретного участника (0-400)
    pub fn set_peer_volume(&self, peer_id: u64, volume: u32) {
        let mut volumes = self.peer_volumes.lock().unwrap();
        volumes.insert(peer_id, volume.min(400));
    }

    /// Получает громкость конкретного участника, либо 100 (дефолт), если его нет в карте
    pub fn get_peer_volume(&self, peer_id: u64) -> u32 {
        let volumes = self.peer_volumes.lock().unwrap();
        volumes.get(&peer_id).copied().unwrap_or(100)
    }

    /// Получает состояние мьют микрофона
    pub fn is_mic_muted(&self) -> bool {
        self.mic_muted.load(Ordering::Relaxed)
    }

    /// Устанавливает состояние мьют микрофона
    pub fn set_mic_muted(&self, muted: bool) {
        self.mic_muted.store(muted, Ordering::Relaxed);
    }

    /// Получает уровень громкости микрофона (0-100)
    pub fn ch1_volume(&self) -> u32 {
        self.ch1_volume.load(Ordering::Relaxed)
    }

    /// Устанавливает уровень громкости микрофона (0-100)
    pub fn set_ch1_volume(&self, volume: u32) {
        self.ch1_volume.store(volume.min(100), Ordering::Relaxed);
    }

    /// Получает уровень громкости гитары (0-100)
    pub fn ch2_volume(&self) -> u32 {
        self.ch2_volume.load(Ordering::Relaxed)
    }

    /// Устанавливает уровень громкости гитары (0-100)
    pub fn set_ch2_volume(&self, volume: u32) {
        self.ch2_volume.store(volume.min(100), Ordering::Relaxed);
    }

    /// Получает уровень громкости собеседников (0-400)
    pub fn output_volume(&self) -> u32 {
        self.output_volume.load(Ordering::Relaxed)
    }

    /// Устанавливает уровень громкости собеседников (0-400)
    pub fn set_output_volume(&self, volume: u32) {
        self.output_volume.store(volume.min(400), Ordering::Relaxed);
    }

    /// Получает текущий пиковый уровень входного сигнала
    pub fn get_input_peak(&self) -> f32 {
        self.input_peak.load(Ordering::Relaxed) as f32 / 100.0
    }
    
    /// Устанавливает сырой пиковый уровень входного сигнала напрямую
    pub fn set_input_peak_raw(&self, peak: u32) {
        self.input_peak.store(peak, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Default for AudioMixer {
    fn default() -> Self {
        Self::new()
    }
}