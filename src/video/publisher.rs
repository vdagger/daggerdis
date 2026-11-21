//! Поток публикации: захват экрана → NV12 на GPU → H.264.
//!
//! Всё тяжёлое живёт в отдельном потоке, а GUI получает готовые
//! закодированные кадры и локально их же декодирует для превью.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::win_dxgi::ScreenCapture;
use super::win_mf::MfEncoder;
use super::{EncodedUnit, VideoPreset, VideoTarget};

pub use super::StreamInfo;

pub struct ScreenPublisher {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    pub rx: Receiver<EncodedUnit>,
    pub info: StreamInfo,
    /// С какого адаптера и в каком размере идёт захват (диагностика).
    pub source: String,
    /// Фактическая частота кодирования за последнюю секунду (диагностика).
    pub encode_fps: Arc<AtomicU32>,
    /// Сколько килобит в секунду уходит в сеть за последнюю секунду (диагностика).
    pub encode_kbps: Arc<AtomicU32>,
}

impl ScreenPublisher {
    pub fn start(fps: u32) -> Result<Self, String> {
        let (init_tx, init_rx) = channel::<Result<(StreamInfo, String), String>>();
        let (frame_tx, frame_rx) = channel::<EncodedUnit>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let encode_fps = Arc::new(AtomicU32::new(0));
        let encode_fps_thread = encode_fps.clone();
        let encode_kbps = Arc::new(AtomicU32::new(0));
        let encode_kbps_thread = encode_kbps.clone();

        let handle = std::thread::Builder::new()
            .name("video-publisher".into())
            .spawn(move || {
                if let Err(e) = run(
                    &stop_thread,
                    fps,
                    &frame_tx,
                    &init_tx,
                    &encode_fps_thread,
                    &encode_kbps_thread,
                ) {
                    eprintln!("[ВИДЕО] Публикация остановлена: {e}");
                }
            })
            .map_err(|e| format!("поток публикации: {e}"))?;

        // ждём инициализации захвата и энкодера, иначе нечего показывать
        match init_rx.recv() {
            Ok(Ok((info, source))) => Ok(Self {
                stop,
                handle: Some(handle),
                rx: frame_rx,
                info,
                source,
                encode_fps,
                encode_kbps,
            }),
            Ok(Err(e)) => {
                let _ = handle.join();
                Err(e)
            }
            Err(_) => Err("поток публикации не ответил при запуске".into()),
        }
    }
}

impl Drop for ScreenPublisher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn run(
    stop: &AtomicBool,
    fps: u32,
    tx: &Sender<EncodedUnit>,
    init: &Sender<Result<(StreamInfo, String), String>>,
    encode_fps: &AtomicU32,
    encode_kbps: &AtomicU32,
) -> Result<(), String> {
    // целимся в нативное разрешение экрана: апскейл тратит битрейт зря
    let mut capture = match ScreenCapture::new(None, fps) {
        Ok(c) => c,
        Err(e) => {
            let _ = init.send(Err(e.clone()));
            return Err(e);
        }
    };

    let (w, h) = capture.dimensions();
    let target = VideoTarget::new(w, h, fps, VideoPreset::default_for(w, h));
    let mut encoder = match MfEncoder::new(&target) {
        Ok(e) => e,
        Err(e) => {
            let _ = init.send(Err(e.clone()));
            return Err(e);
        }
    };

    let info = StreamInfo {
        width: target.width,
        height: target.height,
        fps: target.fps,
        sequence_header: encoder.sequence_header.clone(),
    };
    let source = format!(
        "{} {}x{} → {}x{}",
        capture.adapter_name, capture.source_width, capture.source_height, target.width, target.height
    );
    let _ = init.send(Ok((info, source)));

    let period = Duration::from_secs_f64(1.0 / fps.max(1) as f64);
    let mut next = Instant::now();
    // фактическая скорость кодирования: нужно понять, упирается ли пайплайн
    // в целевой fps или в захват/энкодер
    let mut fps_window = Instant::now();
    let mut fps_frames = 0u32;
    // сколько байт ушло в сеть за окно: битрейт важнее целевого, потому что
    // он и определяет, перегружен ли канал
    let mut window_bytes: u64 = 0;

    while !stop.load(Ordering::Relaxed) {
        let frame = match capture.next_nv12(50) {
            Ok(Some(buf)) => buf,
            Ok(None) => continue, // экран не менялся
            Err(e) => return Err(e),
        };
        match encoder.push_nv12(&frame) {
            Ok(units) => {
                for unit in units {
                    fps_frames += 1;
                    window_bytes += unit.data.len() as u64;
                    if tx.send(unit).is_err() {
                        return Ok(()); // получатель закрылся
                    }
                }
            }
            Err(e) => return Err(e),
        }
        let dt = fps_window.elapsed();
        if dt >= Duration::from_secs(1) {
            let secs = dt.as_secs_f32();
            encode_fps.store((fps_frames as f32 / secs).round() as u32, Ordering::Relaxed);
            // килобиты в секунду: байты × 8 / 1000
            encode_kbps.store(
                (window_bytes as f32 * 8.0 / 1000.0 / secs).round() as u32,
                Ordering::Relaxed,
            );
            fps_window = Instant::now();
            fps_frames = 0;
            window_bytes = 0;
        }

        // не кодируем чаще целевого fps, даже если экран обновляется быстрее
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
    let _ = encoder.finish();
    Ok(())
}


