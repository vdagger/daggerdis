use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;

/// Порог лимитера: −1.0 dBFS = 0.891
const LIMIT_THRESHOLD: f32 = 0.891;

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

/// Подпись ползунка в дБ (для UI громкости участника)
pub fn slider_to_db(val: u32) -> f32 {
    if val == 0 {
        return -30.0;
    }
    20.0 * slider_to_gain(val).log10()
}

/// Обратное преобразование: дБ -> значение ползунка 0..=300
pub fn db_to_slider(db: f32) -> u32 {
    if db <= 0.0 {
        ((10.0f32.powf(db / 20.0)) * 100.0).round().clamp(0.0, 100.0) as u32
    } else {
        (100.0 + (db / 18.0) * 200.0).round().clamp(100.0, 300.0) as u32
    }
}

/// Лимитер без задержки: атака мгновенная (гейн применяется к тому же семплу,
/// поэтому выброса выше порога не бывает), восстановление плавное (~150 мс).
/// Ставится один раз на общий микс — сколько бы собеседников ни говорило
/// одновременно, на выходе не больше −1 dBFS.
pub struct Limiter {
    threshold: f32,
    release_coef: f32,
    gain: f32,
}

impl Limiter {
    pub fn new(sample_rate: u32) -> Self {
        let release_coef = 1.0 - (-1.0f32 / (0.15 * sample_rate as f32)).exp();
        Self {
            threshold: LIMIT_THRESHOLD,
            release_coef,
            gain: 1.0,
        }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        for sample in buf.iter_mut() {
            let peak = sample.abs();
            let needed = if peak > self.threshold {
                self.threshold / peak
            } else {
                1.0
            };
            if needed < self.gain {
                self.gain = needed;
            } else {
                self.gain += (needed - self.gain) * self.release_coef;
            }
            *sample *= self.gain;
        }
    }
}

/// Автоматическая регулировка усиления микрофона: тихий голос подтягивается
/// до цели по RMS, громкий осаживается — как AGC в Discord. Без него тихий
/// микрофон уходил в эфир тихим, и громкость приходилось добирать уже у
/// собеседника на приёме.
pub struct TransmitAgc {
    env_rms: f32,
    gain: f32,
}

impl TransmitAgc {
    /// Цель по RMS: −20 dBFS. Это речь с пиками около −8 dBFS.
    const TARGET_RMS: f32 = 0.10;
    /// Ниже этого RMS не считаем сигнал речью и не трогаем усиление.
    /// Порог низкий намеренно: тихая речь тоже должна подтягиваться, а шум
    /// в паузах до AGC не доходит вовсе — его обнуляет гейт.
    const FLOOR_RMS: f32 = 0.0005;
    /// Диапазон усиления: −12 .. +24 дБ
    const MIN_GAIN: f32 = 0.25;
    const MAX_GAIN: f32 = 15.85;

    pub fn new() -> Self {
        Self {
            env_rms: 0.0,
            gain: 1.0,
        }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        if buf.is_empty() {
            return;
        }
        let rms = (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt();
        let mut target = self.gain;
        if rms > Self::FLOOR_RMS {
            // Огибающая RMS: захват почти мгновенный, спад медленный
            let coef = if rms > self.env_rms { 0.8 } else { 0.1 };
            self.env_rms += (rms - self.env_rms) * coef;
            let desired =
                (Self::TARGET_RMS / self.env_rms.max(1e-4)).clamp(Self::MIN_GAIN, Self::MAX_GAIN);
            // Вниз почти сразу: иначе первая доля громкой фразы проходит
            // некомпрессированной и на слух получается «громко — резко тише».
            // Вверх медленно: так нет «выдоха» между слогами.
            let coef = if desired < self.gain { 0.7 } else { 0.05 };
            target = self.gain + (desired - self.gain) * coef;
        }
        // Усиление меняется не ступенькой на границе буфера, а плавно по нему:
        // иначе на каждом колбэке слышен щелчок (zipper noise).
        let start = self.gain;
        let step = (target - start) / buf.len() as f32;
        for (i, sample) in buf.iter_mut().enumerate() {
            *sample *= start + step * (i as f32 + 1.0);
        }
        self.gain = target;
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
    /// Прослушивание своего тракта в своих же динамиках
    monitor_enabled: AtomicBool,
    /// Обработанный звук своего тракта (48 кГц), ждущий вывода в динамики
    monitor_buf: Mutex<VecDeque<f32>>,
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
            monitor_enabled: AtomicBool::new(false),
            monitor_buf: Mutex::new(VecDeque::new()),
        }
    }

    /// Включено ли прослушивание своего тракта
    pub fn is_monitor_enabled(&self) -> bool {
        self.monitor_enabled.load(Ordering::Relaxed)
    }

    /// Включает/выключает прослушивание. При включении буфер чистится,
    /// иначе в динамики успевает уйти накопленный старый звук.
    pub fn set_monitor_enabled(&self, on: bool) {
        self.monitor_enabled.store(on, Ordering::Relaxed);
        if let Ok(mut buf) = self.monitor_buf.lock() {
            buf.clear();
        }
    }

    /// Кладёт обработанный кусок своего тракта (уже 48 кГц) в буфер
    /// прослушивания. Хвост ограничен 400 мс: если вывод не успевает,
    /// старое выкидывается, а не накапливается.
    pub fn push_monitor(&self, samples: &[f32]) {
        if !self.is_monitor_enabled() {
            return;
        }
        if let Ok(mut buf) = self.monitor_buf.lock() {
            buf.extend(samples.iter().copied());
            const CAP: usize = 48_000 * 4 / 10;
            while buf.len() > CAP {
                buf.pop_front();
            }
        }
    }

    /// Забирает до `max` сэмплов для вывода в динамики
    pub fn drain_monitor(&self, out: &mut Vec<f32>, max: usize) {
        out.clear();
        if let Ok(mut buf) = self.monitor_buf.lock() {
            let n = max.min(buf.len());
            out.extend(buf.drain(..n));
        }
    }

    /// Применяет общий мьют к сведённому входному буферу
    /// (поканальное усиление и VU-пики считаются в аудио-колбэке)
    pub fn process_input(&self, buffer: &mut [f32]) {
        if self.mic_muted.load(Ordering::Relaxed) {
            buffer.fill(0.0);
        }
    }

    /// Применяет громкость участника к его декодированному буферу и замеряет пик.
    /// Общий лимитер здесь не нужен: голоса складываются в один микс-буфер,
    /// ограничивает уже его `Limiter` в конце колбэка вывода — иначе персональный
    /// буст упирался в лимитер собственного голоса и +18 дБ было не слышно.
    pub fn process_output(&self, peer_id: u64, buffer: &mut [f32]) {
        let peer_gain = slider_to_gain(self.get_peer_volume(peer_id));

        let mut peak = 0.0f32;
        for sample in buffer.iter_mut() {
            *sample *= peer_gain;
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

    /// Общая громкость собеседников (коэффициент для сведённого микса)
    pub fn master_gain(&self) -> f32 {
        slider_to_gain(self.output_volume.load(Ordering::Relaxed))
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

    /// Снимок всех per-peer громкостей (для сохранения в конфиг)
    pub fn peer_volumes(&self) -> HashMap<u64, u32> {
        self.peer_volumes.lock().unwrap().clone()
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
