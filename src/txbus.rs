//! Шина передачи звука: входы кладут сэмплы в кольца, один воркер собирает кадр.
//!
//! Раньше энкодер жил в колбэке единственного входного потока. С двумя
//! устройствами у колбэков свои часы: тот, что отдал буфер первым, запускал бы
//! кадр раньше соседнего, и вторая карта навсегда отставала на буфер. Поэтому
//! кадром командует отдельный поток с дедлайном 20 мс, а колбэки только
//! раскладывают свои сэмплы по слотам (усиление, канал и VU — как раньше).
//!
//! Из аудио-колбэка нельзя блокироваться: `push` берёт мьютекс слота на время
//! записи и будит воркер через `try_send`. Энкодер, лимитер и пер-слотовые
//! гейт/AGC живут только в воркере — две карты никогда не кодируют параллельно.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use crate::mixer::{AudioMixer, Limiter, TransmitAgc};

/// Длина кадра Opus: 20 мс при 48 кГц.
pub const FRAME: usize = 960;
/// Жёсткий потолок кольца: 100 мс. Дальше старые сэмплы выкидываются.
const RING_CAP: usize = 4800;
/// Запас, выше которого кольцо подрезается перед сборкой кадра (40 мс).
const HIGH_WATER: usize = 1920;
/// Период кадра.
const FRAME_PERIOD: Duration = Duration::from_millis(20);
/// Слот считается молчащим, если данных не было дольше этого срока.
const SLOT_TIMEOUT: Duration = Duration::from_millis(250);

struct Slot {
    ring: VecDeque<f32>,
    last_push: Option<Instant>,
}

/// Общая шина передачи. Создаётся один раз и раздаётся во все входные потоки.
pub struct InputBus {
    slots: [Mutex<Slot>; 2],
    /// Эпоха слота: колбэк старого потока после перезапуска видит чужую эпоху
    /// и не дописывает в кольцо.
    epoch: [AtomicU64; 2],
    notify: SyncSender<()>,
    notify_rx: Mutex<Option<Receiver<()>>>,
    stop: AtomicBool,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

/// Всё, что нужно входному потоку и воркеру. Клонируется в каждый колбэк.
#[derive(Clone)]
pub struct TxContext {
    pub bus: Arc<InputBus>,
    pub mixer: Arc<AudioMixer>,
    pub ptt_flag: Arc<AtomicBool>,
    pub packet_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
}

impl TxContext {
    pub fn new(
        mixer: Arc<AudioMixer>,
        ptt_flag: Arc<AtomicBool>,
        packet_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    ) -> Self {
        let bus = InputBus::new(mixer.clone(), ptt_flag.clone(), packet_tx.clone());
        Self {
            bus,
            mixer,
            ptt_flag,
            packet_tx,
        }
    }
}

impl InputBus {
    pub fn new(
        mixer: Arc<AudioMixer>,
        ptt_flag: Arc<AtomicBool>,
        packet_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    ) -> Arc<Self> {
        let (notify, notify_rx) = sync_channel(1);
        let bus = Arc::new(Self {
            slots: [Mutex::new(Slot::new()), Mutex::new(Slot::new())],
            epoch: [AtomicU64::new(0), AtomicU64::new(0)],
            notify,
            notify_rx: Mutex::new(Some(notify_rx)),
            stop: AtomicBool::new(false),
            worker: Mutex::new(None),
        });
        let handle = spawn_worker(&bus, mixer, ptt_flag, packet_tx);
        match bus.worker.lock() {
            Ok(mut w) => *w = handle,
            Err(p) => *p.into_inner() = handle,
        }
        bus
    }

    /// Кладёт готовые сэмплы (48 кГц, уже с усилением канала) в слот.
    /// Вызывается из аудио-колбэка — только неблокирующие операции.
    pub fn push(&self, slot: usize, epoch: u64, samples: &[f32]) {
        if slot > 1 || self.epoch[slot].load(Ordering::Relaxed) != epoch {
            return;
        }
        let mut s = match self.slots[slot].lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        s.ring.extend(samples.iter().copied());
        if s.ring.len() > RING_CAP {
            let extra = s.ring.len() - RING_CAP;
            s.ring.drain(..extra);
        }
        s.last_push = Some(Instant::now());
        drop(s);
        let _ = self.notify.try_send(());
    }

    pub fn epoch(&self, slot: usize) -> u64 {
        self.epoch[slot].load(Ordering::Relaxed)
    }

    /// Эпохи для нового потока — ещё не применённые. Порядок важен: новый
    /// поток собирается со свежей эпохой, но старый продолжает писать, пока
    /// сборка не удалась. `commit_epochs` вызывается только после успеха.
    pub fn next_epochs(&self, fill: [bool; 2]) -> [u64; 2] {
        let mut epochs = [self.epoch(0), self.epoch(1)];
        for slot in 0..2 {
            if fill[slot] {
                epochs[slot] += 1;
            }
        }
        epochs
    }

    /// Колбэки старого потока со старой эпохой больше не пишут в слот.
    pub fn commit_epochs(&self, fill: [bool; 2], epochs: [u64; 2]) {
        for slot in 0..2 {
            if fill[slot] {
                self.epoch[slot].store(epochs[slot], Ordering::Relaxed);
            }
        }
    }

    /// Забирает до кадра сэмплов из слота. `false` — слот молчит.
    fn take(&self, slot: usize, out: &mut Vec<f32>) -> bool {
        out.clear();
        let mut s = match self.slots[slot].lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let fresh = s
            .last_push
            .map_or(false, |t| t.elapsed() < SLOT_TIMEOUT);
        if !fresh {
            s.ring.clear();
            return false;
        }
        // Подрезаем застой: после паузы планировщика не ждём, пока кольцо
        // доиграет старые 100 мс.
        if s.ring.len() > HIGH_WATER {
            let extra = s.ring.len() - HIGH_WATER;
            s.ring.drain(..extra);
        }
        let n = s.ring.len().min(FRAME);
        out.extend(s.ring.drain(..n));
        true
    }
}

impl Drop for InputBus {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.notify.try_send(());
        let handle = match self.worker.lock() {
            Ok(mut w) => w.take(),
            Err(p) => p.into_inner().take(),
        };
        // Шина может умереть в самом воркере (последняя ссылка всплыла из Weak) —
        // join собственного потока не делаем.
        if let Some(h) = handle {
            if h.thread().id() != std::thread::current().id() {
                let _ = h.join();
            }
        }
    }
}

impl Slot {
    fn new() -> Self {
        Self {
            ring: VecDeque::with_capacity(FRAME * 2),
            last_push: None,
        }
    }
}

fn spawn_worker(
    bus: &Arc<InputBus>,
    mixer: Arc<AudioMixer>,
    ptt_flag: Arc<AtomicBool>,
    packet_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
) -> Option<std::thread::JoinHandle<()>> {
    let weak: Weak<InputBus> = Arc::downgrade(bus);
    let rx = bus.notify_rx.lock().ok()?.take()?;
    std::thread::Builder::new()
        .name("albus-tx".into())
        .spawn(move || {
            let mut worker = match TxWorker::new() {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("[ОШИБКА] Не удалось создать Opus-энкодер: {}", e);
                    return;
                }
            };
            let mut next = Instant::now();
            loop {
                let now = Instant::now();
                if now < next {
                    // Колбэк будит раньше дедлайна — кадр собираем строго по часам.
                    let wait = (next - now).min(Duration::from_millis(10));
                    let _ = rx.recv_timeout(wait);
                }
                let Some(bus) = weak.upgrade() else { break };
                if bus.stop.load(Ordering::Relaxed) {
                    break;
                }
                let now = Instant::now();
                if now >= next {
                    let ptt = ptt_flag.load(Ordering::Relaxed);
                    if let Some(packet) = worker.render_frame(&bus, &mixer, ptt) {
                        let _ = packet_tx.try_send(packet);
                    }
                    next += FRAME_PERIOD;
                    // Отстали больше чем на кадр (сон, отладчик) — догонять
                    // пачкой кадров нельзя, начинаем заново.
                    if next < now {
                        next = now + FRAME_PERIOD;
                    }
                }
            }
        })
        .ok()
}

/// Сборка кадра из колец. Отдельно от потока — чтобы тестировать без часов.
pub(crate) struct TxWorker {
    encoder: rusty_opus::OpusEncoder,
    limiter: Limiter,
    agcs: [TransmitAgc; 2],
    gates: [f32; 2],
    held: [f32; 2],
    have_held: [bool; 2],
    slot_buf: Vec<f32>,
    mixed: Vec<f32>,
    packet_buf: Vec<u8>,
}

impl TxWorker {
    pub(crate) fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let mut encoder = rusty_opus::OpusEncoder::new(48000, 1, rusty_opus::Application::Voip)?;
        encoder.bitrate_bps = 64_000;
        Ok(Self {
            encoder,
            limiter: Limiter::new(48_000),
            agcs: [TransmitAgc::new(), TransmitAgc::new()],
            gates: [0.0, 0.0],
            held: [0.0, 0.0],
            have_held: [false, false],
            slot_buf: Vec::with_capacity(FRAME),
            mixed: Vec::with_capacity(FRAME),
            packet_buf: Vec::new(),
        })
    }

    pub(crate) fn render_frame(
        &mut self,
        bus: &InputBus,
        mixer: &AudioMixer,
        ptt: bool,
    ) -> Option<Vec<u8>> {
        let dsp = mixer.is_dsp_enabled();
        self.mixed.clear();
        self.mixed.resize(FRAME, 0.0);
        let mut contributing = 0u32;

        for slot in 0..2 {
            let (gain, profile) = if slot == 0 {
                (mixer.ch1_volume(), mixer.input1_profile())
            } else {
                (mixer.ch2_volume(), mixer.input2_profile())
            };

            if !bus.take(slot, &mut self.slot_buf) {
                // Поток молчит: VU гасим, а не оставляем замершим на последнем
                // значении, и не тянем хвост прошлого звука в тишину.
                if slot == 0 {
                    mixer.set_input1_peak_raw(0);
                } else {
                    mixer.set_input2_peak_raw(0);
                }
                self.held[slot] = 0.0;
                self.have_held[slot] = false;
                continue;
            }

            // Недобор (часы карт разъехались или колбэк опоздал) — держим
            // последний сэмпл: вставленный посреди сигнала ноль слышен щелчком.
            if self.slot_buf.len() < FRAME {
                // Добиваем последним сэмплом именно этого куска; `held` нужен,
                // когда слот в этом кадре не дал ни одного нового сэмпла.
                let fill = match self.slot_buf.last() {
                    Some(&last) => last,
                    None if self.have_held[slot] => self.held[slot],
                    None => 0.0,
                };
                self.slot_buf.resize(FRAME, fill);
            }
            if let Some(&last) = self.slot_buf.last() {
                self.held[slot] = last;
                self.have_held[slot] = true;
            }

            if dsp && profile.uses_voice_dsp() {
                self.apply_voice_dsp(slot);
            }

            if gain > 0 {
                for (dst, src) in self.mixed.iter_mut().zip(self.slot_buf.iter()) {
                    *dst += *src;
                }
                contributing += 1;
            }
        }

        // Нормировка по числу звучащих входов — как раньше с двумя каналами
        // одной карты: два входа на 100 % не должны клиппиться в сумме.
        if contributing > 1 {
            let n = contributing as f32;
            for s in self.mixed.iter_mut() {
                *s /= n;
            }
        }

        mixer.process_input(&mut self.mixed);
        // Лимитер — часть обработки микрофона: при выключенной обработке
        // уровень должен быть ровно таким, каким его выставили.
        if dsp {
            self.limiter.process(&mut self.mixed);
        }
        // Прослушивание своего тракта — всегда, независимо от PTT: баланс
        // входов настраивается до эфира.
        mixer.push_monitor(&self.mixed);

        if !ptt {
            return None;
        }
        self.packet_buf.clear();
        self.packet_buf.resize(1024, 0);
        match self.encoder.encode(&self.mixed, FRAME, &mut self.packet_buf) {
            Ok(len) => Some(self.packet_buf[..len].to_vec()),
            Err(_) => None,
        }
    }

    /// Затвор + AGC — только для голосового профиля. Коэффициенты вполовину
    /// мягче прежних: старый колбэк обрабатывал ~480 сэмплов, здесь 960.
    fn apply_voice_dsp(&mut self, slot: usize) {
        let chunk_peak = self
            .slot_buf
            .iter()
            .map(|s| s.abs())
            .fold(0.0f32, f32::max);
        let target = if chunk_peak > 0.006 { 1.0f32 } else { 0.0f32 };
        let mut gain = self.gates[slot];
        for sample in self.slot_buf.iter_mut() {
            if target > gain {
                gain += (target - gain) * 0.05;
            } else {
                gain += (target - gain) * 0.001;
            }
            *sample *= gain;
        }
        self.gates[slot] = gain;
        self.agcs[slot].process(&mut self.slot_buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mixer::SlotProfile;

    /// Шина без воркера: кадры собираем вручную, ничего не тикает параллельно.
    fn silent_bus() -> Arc<InputBus> {
        Arc::new(InputBus {
            slots: [Mutex::new(Slot::new()), Mutex::new(Slot::new())],
            epoch: [AtomicU64::new(0), AtomicU64::new(0)],
            notify: sync_channel(1).0,
            notify_rx: Mutex::new(None),
            stop: AtomicBool::new(false),
            worker: Mutex::new(None),
        })
    }

    fn worker(mixer: &AudioMixer) -> TxWorker {
        mixer.set_monitor_enabled(true);
        TxWorker::new().expect("энкодер")
    }

    fn monitor(mixer: &AudioMixer) -> Vec<f32> {
        let mut out = Vec::new();
        mixer.drain_monitor(&mut out, FRAME);
        out
    }

    #[test]
    fn two_slots_sum_and_normalise() {
        let bus = silent_bus();
        let mixer = AudioMixer::new();
        let mut w = worker(&mixer);
        bus.push(0, bus.epoch(0), &vec![0.5; FRAME]);
        bus.push(1, bus.epoch(1), &vec![0.5; FRAME]);
        w.render_frame(&bus, &mixer, false);
        let out = monitor(&mixer);
        assert_eq!(out.len(), FRAME);
        // (0.5 + 0.5) / 2 = 0.5 — сумма двух входов не удваивает уровень.
        assert!((out[0] - 0.5).abs() < 1e-6, "{}", out[0]);
    }

    #[test]
    fn inactive_slot_does_not_halve() {
        let bus = silent_bus();
        let mixer = AudioMixer::new();
        let mut w = worker(&mixer);
        bus.push(0, bus.epoch(0), &vec![0.5; FRAME]);
        w.render_frame(&bus, &mixer, false);
        assert!((monitor(&mixer)[0] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn underrun_holds_last_sample() {
        let bus = silent_bus();
        let mixer = AudioMixer::new();
        let mut w = worker(&mixer);
        bus.push(0, bus.epoch(0), &vec![0.4; 480]);
        w.render_frame(&bus, &mixer, false);
        let out = monitor(&mixer);
        // Первая половина — реальные сэмплы, вторая — повтор последнего,
        // а не нули посреди сигнала.
        assert!(out.iter().all(|s| (*s - 0.4).abs() < 1e-6), "{:?}", &out[..4]);
    }

    #[test]
    fn empty_slot_is_silence() {
        let bus = silent_bus();
        let mixer = AudioMixer::new();
        let mut w = worker(&mixer);
        w.render_frame(&bus, &mixer, false);
        assert!(monitor(&mixer).iter().all(|s| *s == 0.0));
    }

    #[test]
    fn overflow_trims_ring() {
        let bus = silent_bus();
        bus.push(0, bus.epoch(0), &vec![0.1; 6000]);
        let len = bus.slots[0].lock().unwrap().ring.len();
        assert_eq!(len, RING_CAP);
    }

    #[test]
    fn stale_slot_falls_silent_and_zeroes_vu() {
        let bus = silent_bus();
        let mixer = AudioMixer::new();
        let mut w = worker(&mixer);
        bus.push(0, bus.epoch(0), &vec![0.5; FRAME]);
        mixer.set_input1_peak_raw(50);
        // Состарили слот, будто поток умер
        bus.slots[0].lock().unwrap().last_push =
            Some(Instant::now() - SLOT_TIMEOUT - Duration::from_millis(1));
        w.render_frame(&bus, &mixer, false);
        assert_eq!(mixer.input1_peak(), 0.0);
        assert!(monitor(&mixer).iter().all(|s| *s == 0.0));
    }

    #[test]
    fn mute_zeroes_frame() {
        let bus = silent_bus();
        let mixer = AudioMixer::new();
        let mut w = worker(&mixer);
        mixer.set_mic_muted(true);
        bus.push(0, bus.epoch(0), &vec![0.5; FRAME]);
        w.render_frame(&bus, &mixer, false);
        assert!(monitor(&mixer).iter().all(|s| *s == 0.0));
    }

    #[test]
    fn limiter_only_when_dsp_enabled() {
        let bus = silent_bus();
        let mixer = AudioMixer::new();
        let mut w = worker(&mixer);
        // Один вход на 0.95 при выключенной обработке остаётся 0.95.
        bus.push(0, bus.epoch(0), &vec![0.95; FRAME]);
        w.render_frame(&bus, &mixer, false);
        assert!(monitor(&mixer)[0] > 0.9);

        // С обработкой тот же вход опускается под −1 dBFS.
        let bus2 = silent_bus();
        let mixer2 = AudioMixer::new();
        let mut w2 = worker(&mixer2);
        mixer2.set_dsp_enabled(true);
        bus2.push(0, bus2.epoch(0), &vec![0.95; FRAME]);
        w2.render_frame(&bus2, &mixer2, false);
        assert!(monitor(&mixer2)[0] <= 0.891 + 1e-6);
    }

    #[test]
    fn instrument_profile_skips_gate_and_agc() {
        let bus = silent_bus();
        let mixer = AudioMixer::new();
        let mut w = worker(&mixer);
        mixer.set_dsp_enabled(true);
        mixer.set_input1_profile(SlotProfile::Instrument);
        // Тихий инструмент проходит как есть: ни затвор его не режет,
        // ни AGC не разгоняет.
        bus.push(0, bus.epoch(0), &vec![0.002; FRAME]);
        w.render_frame(&bus, &mixer, false);
        let out = monitor(&mixer);
        assert!((out[0] - 0.002).abs() < 1e-5, "{}", out[0]);
        assert!(!SlotProfile::Instrument.uses_voice_dsp());
    }

    #[test]
    fn ptt_off_still_drains_rings() {
        let bus = silent_bus();
        let mixer = AudioMixer::new();
        let mut w = worker(&mixer);
        bus.push(0, bus.epoch(0), &vec![0.3; FRAME * 2]);
        assert!(w.render_frame(&bus, &mixer, false).is_none());
        let left = bus.slots[0].lock().unwrap().ring.len();
        assert_eq!(left, FRAME, "кадр должен уйти в монитор без PTT");
    }

    #[test]
    fn ptt_sends_one_packet() {
        let bus = silent_bus();
        let mixer = AudioMixer::new();
        let mut w = worker(&mixer);
        bus.push(0, bus.epoch(0), &vec![0.3; FRAME]);
        assert!(w.render_frame(&bus, &mixer, true).is_some());
    }

    #[test]
    fn old_epoch_push_is_ignored() {
        let bus = silent_bus();
        let stale = bus.epoch(0);
        let next = bus.next_epochs([true, false]);
        assert_eq!(next[0], stale + 1);
        bus.commit_epochs([true, false], next);
        bus.push(0, stale, &vec![0.5; FRAME]);
        assert_eq!(bus.slots[0].lock().unwrap().ring.len(), 0);
        bus.push(0, next[0], &vec![0.5; FRAME]);
        assert_eq!(bus.slots[0].lock().unwrap().ring.len(), FRAME);
    }
}
