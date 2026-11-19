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

/// Профиль обработки входа: голосу нужны шумодав, затвор и AGC, инструменту
/// они вредят (AGC осаживает удары и «дышит» на затухающих нотах, RNNoise
/// съедает хвосты идеально ровных нот).
///
/// Профиль — только пресет для трёх тумблеров: «Голос» включает все три,
/// «Инструмент» выключает все. Если тумблеры выставлены вручную не тройкой,
/// профиль становится `Custom` — так в настройках видно, что пресет изменён.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlotProfile {
    Voice = 0,
    Instrument = 1,
    Custom = 2,
}

impl SlotProfile {
    pub fn from_u32(v: u32) -> Self {
        match v {
            1 => Self::Instrument,
            2 => Self::Custom,
            _ => Self::Voice,
        }
    }

    pub fn as_u32(self) -> u32 {
        self as u32
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Voice => "Голос",
            Self::Instrument => "Инструмент",
            Self::Custom => "Свой",
        }
    }

    /// Эффекты — только для голоса: инструмент с затвором теряет
    /// затухающие ноты, а AGC выравнивает аккорды по громкости.
    pub fn uses_voice_dsp(self) -> bool {
        self == Self::Voice
    }

    /// Пресет тумблеров: `Voice` — все включены, `Instrument` — все выключены.
    /// Порядок: (шумодав, затвор, AGC).
    pub fn effects(self) -> (bool, bool, bool) {
        match self {
            Self::Voice => (true, true, true),
            Self::Instrument | Self::Custom => (false, false, false),
        }
    }

    /// Профиль по состоянию тумблеров; `Custom` — когда они не тройка.
    pub fn from_effects(denoise: bool, gate: bool, agc: bool) -> Self {
        match (denoise, gate, agc) {
            (true, true, true) => Self::Voice,
            (false, false, false) => Self::Instrument,
            _ => Self::Custom,
        }
    }
}

/// Аудио-микшер для управления громкостью и мьютом
#[derive(Debug)]
pub struct AudioMixer {
    mic_muted: AtomicBool,
    ch1_volume: AtomicU32, // Вход 1
    ch2_volume: AtomicU32, // Вход 2
    input1_channel: AtomicU32,
    input2_channel: AtomicU32,
    /// Эффекты входа 1: шумодав, затвор и AGC — независимо друг от друга.
    input1_denoise: AtomicBool,
    input1_gate: AtomicBool,
    input1_agc: AtomicBool,
    input2_denoise: AtomicBool,
    input2_gate: AtomicBool,
    input2_agc: AtomicBool,
    /// Эхо-подавление AEC3 на входе — отдельный тумблер, не часть профилей:
    /// нужен только тем, кто слушает собеседников в колонках.
    input1_aec: AtomicBool,
    input2_aec: AtomicBool,
    output_volume: AtomicU32,
    input1_peak: AtomicU32,
    input2_peak: AtomicU32,
    peer_volumes: Mutex<HashMap<u64, u32>>,
    peer_meters: Mutex<HashMap<u64, PeerMeter>>,
    /// Прослушивание своего тракта в своих же динамиках
    monitor_enabled: AtomicBool,
    /// Обработанный звук своего тракта (48 кГц), ждущий вывода в динамики
    monitor_buf: Mutex<VecDeque<f32>>,
    /// Опорный сигнал эхо-подавления (48 кГц): то, что реально уходит
    /// в динамики из голосов собеседников. Монитор и гудок сюда не входят.
    echo_ref: Mutex<VecDeque<f32>>,
    /// Гудок звонка: 0 — тихо, 1 — входящий, 2 — «звоним…»
    ring_mode: AtomicU32,
    /// Лимитер на передаваемом миксе. Раньше он жил под общим тумблером
    /// «обработка звука» вместе с затвором и AGC — теперь включается сам:
    /// страховка от клиппинга не должна требовать ещё и гейта в тракте.
    /// По умолчанию выключен: уровень ровно такой, каким его выставили.
    tx_limiter_enabled: AtomicBool,
    /// Страховочный лимитер на общем миксе собеседников. Тоже по умолчанию
    /// выключен: без него громкость приёма ровно такая, какую выставили.
    rx_limiter_enabled: AtomicBool,
    /// Битрейт Opus на передачу: 64 кбит/с хватает речи, инструменту мало.
    opus_bitrate: AtomicU32,
}

/// Допустимые битрейты передачи. Значение вне набора не даёт ничего, кроме
/// непредсказуемого качества, поэтому и в конфиге, и в UI — только эти три.
pub const OPUS_BITRATES: [u32; 3] = [64_000, 96_000, 128_000];

impl AudioMixer {
    /// Создает новый аудио-микшер с дефолтными значениями
    pub fn new() -> Self {
        Self {
            mic_muted: AtomicBool::new(false),
            ch1_volume: AtomicU32::new(100),
            ch2_volume: AtomicU32::new(100),
            input1_channel: AtomicU32::new(0),
            input2_channel: AtomicU32::new(1),
            input1_denoise: AtomicBool::new(false),
            input1_gate: AtomicBool::new(false),
            input1_agc: AtomicBool::new(false),
            input2_denoise: AtomicBool::new(false),
            input2_gate: AtomicBool::new(false),
            input2_agc: AtomicBool::new(false),
            input1_aec: AtomicBool::new(false),
            input2_aec: AtomicBool::new(false),
            output_volume: AtomicU32::new(100),
            input1_peak: AtomicU32::new(0),
            input2_peak: AtomicU32::new(0),
            peer_volumes: Mutex::new(HashMap::new()),
            peer_meters: Mutex::new(HashMap::new()),
            monitor_enabled: AtomicBool::new(false),
            monitor_buf: Mutex::new(VecDeque::new()),
            echo_ref: Mutex::new(VecDeque::new()),
            ring_mode: AtomicU32::new(0),
            tx_limiter_enabled: AtomicBool::new(false),
            rx_limiter_enabled: AtomicBool::new(false),
            opus_bitrate: AtomicU32::new(64_000),
        }
    }

    /// Битрейт Opus на передачу (бит/с)
    pub fn opus_bitrate(&self) -> u32 {
        self.opus_bitrate.load(Ordering::Relaxed)
    }

    /// Ближайший допустимый битрейт: значение из настроек может быть старым
    /// или испорченным, а энкодеру нужно одно из трёх.
    pub fn set_opus_bitrate(&self, bps: u32) {
        let nearest = OPUS_BITRATES
            .iter()
            .copied()
            .min_by_key(|v| v.abs_diff(bps))
            .unwrap_or(64_000);
        self.opus_bitrate.store(nearest, Ordering::Relaxed);
    }

    /// Включён ли лимитер на передаваемом миксе
    pub fn is_tx_limiter_enabled(&self) -> bool {
        self.tx_limiter_enabled.load(Ordering::Relaxed)
    }

    pub fn set_tx_limiter_enabled(&self, on: bool) {
        self.tx_limiter_enabled.store(on, Ordering::Relaxed);
    }

    /// Шумодав RNNoise на входе 1: убирает клавиатуру, кулеры и улицу,
    /// которые затвор пропускает вместе с речью.
    pub fn input1_denoise(&self) -> bool {
        self.input1_denoise.load(Ordering::Relaxed)
    }

    pub fn set_input1_denoise(&self, on: bool) {
        self.input1_denoise.store(on, Ordering::Relaxed);
    }

    pub fn input2_denoise(&self) -> bool {
        self.input2_denoise.load(Ordering::Relaxed)
    }

    pub fn set_input2_denoise(&self, on: bool) {
        self.input2_denoise.store(on, Ordering::Relaxed);
    }

    pub fn input1_gate(&self) -> bool {
        self.input1_gate.load(Ordering::Relaxed)
    }

    pub fn set_input1_gate(&self, on: bool) {
        self.input1_gate.store(on, Ordering::Relaxed);
    }

    pub fn input1_agc(&self) -> bool {
        self.input1_agc.load(Ordering::Relaxed)
    }

    pub fn set_input1_agc(&self, on: bool) {
        self.input1_agc.store(on, Ordering::Relaxed);
    }

    pub fn input2_gate(&self) -> bool {
        self.input2_gate.load(Ordering::Relaxed)
    }

    pub fn set_input2_gate(&self, on: bool) {
        self.input2_gate.store(on, Ordering::Relaxed);
    }

    pub fn input2_agc(&self) -> bool {
        self.input2_agc.load(Ordering::Relaxed)
    }

    pub fn set_input2_agc(&self, on: bool) {
        self.input2_agc.store(on, Ordering::Relaxed);
    }

    /// Эхо-подавление на входе 1: вычитает из микрофона голос собеседников,
    /// который слышат колонки.
    pub fn input1_aec(&self) -> bool {
        self.input1_aec.load(Ordering::Relaxed)
    }

    pub fn set_input1_aec(&self, on: bool) {
        self.input1_aec.store(on, Ordering::Relaxed);
    }

    pub fn input2_aec(&self) -> bool {
        self.input2_aec.load(Ordering::Relaxed)
    }

    pub fn set_input2_aec(&self, on: bool) {
        self.input2_aec.store(on, Ordering::Relaxed);
    }

    /// Кладёт кусок опоры эхо-подавления (48 кГц) из колбэка вывода.
    /// Сэмплы за пределами −1..1 и NaN/бесконечность недопустимы для AEC3,
    /// поэтому приводим их здесь, а не в воркере передачи.
    pub fn push_echo_ref(&self, samples: &[f32]) {
        if let Ok(mut buf) = self.echo_ref.lock() {
            buf.extend(samples.iter().map(|s| {
                if s.is_finite() {
                    s.clamp(-1.0, 1.0)
                } else {
                    0.0
                }
            }));
            // Полсекунды: при отставании воркера старое выкидывается, а не копится.
            const CAP: usize = 48_000 / 2;
            while buf.len() > CAP {
                buf.pop_front();
            }
        }
    }

    /// Забирает опору на кадр передачи; недобор добивается нулями.
    pub fn drain_echo_ref(&self, out: &mut [f32]) {
        out.fill(0.0);
        if let Ok(mut buf) = self.echo_ref.lock() {
            let n = out.len().min(buf.len());
            for (dst, src) in out.iter_mut().zip(buf.drain(..n)) {
                *dst = src;
            }
        }
    }

    /// Включён ли лимитер на общем миксе собеседников
    pub fn is_rx_limiter_enabled(&self) -> bool {
        self.rx_limiter_enabled.load(Ordering::Relaxed)
    }

    pub fn set_rx_limiter_enabled(&self, on: bool) {
        self.rx_limiter_enabled.store(on, Ordering::Relaxed);
    }

    /// Режим гудка: 0 — тихо, 1 — входящий звонок, 2 — гудок «звоним…»
    pub fn ring_mode(&self) -> u32 {
        self.ring_mode.load(Ordering::Relaxed)
    }

    pub fn set_ring_mode(&self, mode: u32) {
        self.ring_mode.store(mode, Ordering::Relaxed);
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

    /// Профиль Входа 1, выведенный из тумблеров: «Голос» (оба включены),
    /// «Инструмент» (оба выключены) или «Свой».
    pub fn input1_profile(&self) -> SlotProfile {
        SlotProfile::from_effects(self.input1_denoise(), self.input1_gate(), self.input1_agc())
    }

    /// Пресет: «Голос» включает шумодав, затвор и AGC, «Инструмент» выключает все.
    pub fn set_input1_profile(&self, profile: SlotProfile) {
        let (denoise, gate, agc) = profile.effects();
        self.set_input1_denoise(denoise);
        self.set_input1_gate(gate);
        self.set_input1_agc(agc);
    }

    /// Профиль Входа 2
    pub fn input2_profile(&self) -> SlotProfile {
        SlotProfile::from_effects(self.input2_denoise(), self.input2_gate(), self.input2_agc())
    }

    pub fn set_input2_profile(&self, profile: SlotProfile) {
        let (denoise, gate, agc) = profile.effects();
        self.set_input2_denoise(denoise);
        self.set_input2_gate(gate);
        self.set_input2_agc(agc);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_curve() {
        assert_eq!(slider_to_gain(0), 0.0);
        assert!((slider_to_gain(100) - 1.0).abs() < 1e-6);
        assert!((slider_to_gain(200) - 10f32.powf(9.0 / 20.0)).abs() < 1e-4);
        assert!((slider_to_gain(300) - 10f32.powf(18.0 / 20.0)).abs() < 1e-3);
    }

    #[test]
    fn slider_db_roundtrip() {
        for v in (1..=300).step_by(5) {
            let back = db_to_slider(slider_to_db(v)) as i32;
            assert!((back - v as i32).abs() <= 1, "v={v} back={back}");
        }
        assert_eq!(db_to_slider(-40.0), 1);
    }

    #[test]
    fn limiter_ceiling() {
        let mut lim = Limiter::new(48_000);
        let mut buf = vec![0.95f32; 480];
        lim.process(&mut buf);
        for s in &buf {
            assert!(s.abs() <= LIMIT_THRESHOLD + 1e-6, "выброс выше порога: {s}");
        }
    }

    #[test]
    fn agc_boosts_quiet_signal() {
        // На каждом шаге — свежий блок одной амплитуды: старый тест прогонял
        // один и тот же буфер по кругу, и накопленное усиление умножалось
        // снова и снова, пока значения не уходили в бесконечность.
        let mut agc = TransmitAgc::new();
        let mut buf = Vec::new();
        for _ in 0..400 {
            buf = vec![0.002f32; 960];
            agc.process(&mut buf);
        }
        let rms = (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt();
        // Вход −54 dBFS, потолок усиления +24 дБ (×15.85) → максимум ~0.0317.
        assert!(rms > 0.02, "тихий сигнал не подтянулся: rms={rms}");
    }

    #[test]
    fn effects_off_by_default() {
        let m = AudioMixer::new();
        assert!(
            !m.input1_denoise() && !m.input1_gate() && !m.input1_agc(),
            "вход 1 должен быть сырым"
        );
        assert!(
            !m.input2_denoise() && !m.input2_gate() && !m.input2_agc(),
            "вход 2 должен быть сырым"
        );
        assert!(
            !m.input1_aec() && !m.input2_aec(),
            "эхо-подавление по умолчанию выключено"
        );
        assert!(!m.is_tx_limiter_enabled(), "лимитер передачи должен быть выключен");
        assert!(!m.is_rx_limiter_enabled(), "лимитер приёма должен быть выключен");
        m.set_input1_gate(true);
        m.set_tx_limiter_enabled(true);
        m.set_rx_limiter_enabled(true);
        assert!(m.input1_gate());
        assert!(m.is_tx_limiter_enabled());
        assert!(m.is_rx_limiter_enabled());
    }

    #[test]
    fn opus_bitrate_default_and_snap() {
        let m = AudioMixer::new();
        assert_eq!(m.opus_bitrate(), 64_000);
        m.set_opus_bitrate(97_000);
        assert_eq!(m.opus_bitrate(), 96_000, "округляем до допустимого");
        m.set_opus_bitrate(400_000);
        assert_eq!(m.opus_bitrate(), 128_000, "потолок — 128 кбит/с");
        m.set_opus_bitrate(0);
        assert_eq!(m.opus_bitrate(), 64_000, "пол — 64 кбит/с");
    }

    #[test]
    fn peer_volume_clamped() {
        let m = AudioMixer::new();
        assert_eq!(m.get_peer_volume(7), 100);
        m.set_peer_volume(7, 9999);
        assert_eq!(m.get_peer_volume(7), 300);
    }

    #[test]
    fn echo_ref_clamps_and_pads_with_zeros() {
        let m = AudioMixer::new();
        // AEC3 принимает только конечные сэмплы в −1..1: обрезаем здесь.
        m.push_echo_ref(&[0.5, 2.0, f32::NAN, -1.5]);
        let mut out = [9.0f32; 6];
        m.drain_echo_ref(&mut out);
        assert_eq!(&out[..4], &[0.5, 1.0, 0.0, -1.0]);
        assert_eq!(&out[4..], &[0.0, 0.0], "недобор — тишина");
    }

    #[test]
    fn echo_ref_ring_is_bounded() {
        let m = AudioMixer::new();
        m.push_echo_ref(&vec![0.1; 60_000]);
        let len = m.echo_ref.lock().unwrap().len();
        assert_eq!(len, 48_000 / 2, "старое выкидывается, а не копится");
    }

    #[test]
    fn profile_presets_and_custom() {
        let m = AudioMixer::new();
        // По умолчанию эффекты выключены — профиль выходит «Инструмент».
        assert_eq!(m.input1_profile(), SlotProfile::Instrument);
        m.set_input1_profile(SlotProfile::Voice);
        assert!(m.input1_denoise() && m.input1_gate() && m.input1_agc());
        assert_eq!(m.input1_profile(), SlotProfile::Voice);
        m.set_input2_profile(SlotProfile::Voice);
        assert_eq!(m.input2_profile(), SlotProfile::Voice);
        // Ручной тумблер ломает пару — профиль больше не пресет.
        m.set_input1_agc(false);
        assert_eq!(m.input1_profile(), SlotProfile::Custom);
        assert_eq!(m.input1_profile().label(), "Свой");
        m.set_input1_aec(true);
        m.set_input1_profile(SlotProfile::Instrument);
        assert!(!m.input1_denoise() && !m.input1_gate() && !m.input1_agc());
        assert!(
            m.input1_aec(),
            "пресеты эффектов не должны трогать эхо-подавление"
        );
        // Мусор в конфиге не должен давать произвольного профиля.
        assert_eq!(SlotProfile::from_u32(42), SlotProfile::Voice);
        assert_eq!(SlotProfile::from_u32(2), SlotProfile::Custom);
    }
}
