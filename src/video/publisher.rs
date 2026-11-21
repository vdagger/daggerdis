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

/// Ответ потока публикации на старте: описание потока, подпись источника,
/// битрейт пресета и фактический пресет (для карточки и журнала).
type InitReport = Result<(StreamInfo, String, u32, VideoPreset), String>;

use super::win_wgc::WindowCapture;

/// Откуда берём кадры: весь экран (или его область) либо одно окно через WGC.
enum Capture {
    Screen(ScreenCapture),
    Wgc(WindowCapture),
}

impl Capture {
    fn next_nv12(&mut self, timeout_ms: u32) -> Result<Option<Vec<u8>>, String> {
        match self {
            Capture::Screen(c) => c.next_nv12(timeout_ms),
            Capture::Wgc(c) => c.next_nv12(timeout_ms),
        }
    }

}

/// Собирает источник кадров: окно через WGC, окно вырезкой из экрана или
/// весь экран. Возвращает источник, размер и подпись для карточки.
fn make_capture(
    fps: u32,
    window: Option<isize>,
) -> Result<(Capture, u32, u32, String), String> {
    if let Some(hwnd) = window {
        if super::win_wgc::supported() {
            match WindowCapture::new(hwnd, fps) {
                Ok(c) => {
                    let (w, h) = c.dimensions();
                    return Ok((Capture::Wgc(c), w, h, format!("окно {w}x{h} (WGC)")));
                }
                Err(e) => println!("[ВИДЕО] WGC не поднялся ({e}) — вырезаю окно из кадра экрана"),
            }
        } else {
            println!("[ВИДЕО] WGC недоступен — вырезаю окно из кадра экрана");
        }
        let mut cap = ScreenCapture::new(None, fps)?;
        let desktop = cap.desktop_size();
        let rect = super::win_windows::window_rect(hwnd)
            .and_then(|r| super::win_windows::clamp_rect(r, desktop));
        let Some((x, y, w, h)) = rect else {
            return Err("окно не видно на экране (свёрнуто или уведено за край)".into());
        };
        cap.set_source_rect(Some((x, y, w, h)));
        let label = format!("{} окно {w}x{h}", cap.adapter_name);
        return Ok((Capture::Screen(cap), w, h, label));
    }
    let cap = ScreenCapture::new(None, fps)?;
    let (w, h) = cap.dimensions();
    let label = format!(
        "{} {}x{}",
        cap.adapter_name, cap.source_width, cap.source_height
    );
    Ok((Capture::Screen(cap), w, h, label))
}

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
    /// Целевой битрейт на ходу (0 — не трогать): сюда пишет адаптация канала
    /// из `VideoLink`, энкодер применяет значение в своём цикле.
    pub bitrate: Arc<AtomicU32>,
    /// Битрейт пресета — верхняя граница, к которой адаптация возвращается.
    pub bitrate_base: u32,
    /// Пресет, с которым запущен поток (диагностика и карточка).
    pub preset: VideoPreset,
    /// Просьба выдать IDR: сюда пишет `VideoLink`, когда подписался новый
    /// зритель или зритель потерял опору. `false` — сбросить нечего.
    pub keyframe: Arc<AtomicBool>,
}

impl ScreenPublisher {
    /// `window` — окно, которое транслируем вместо всего экрана: его
    /// прямоугольник вырезается из кадра рабочего стола.
    /// `preset` — качество из настроек; `None` — по разрешению источника.
    pub fn start(
        fps: u32,
        window: Option<isize>,
        preset: Option<VideoPreset>,
        keyframe: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let (init_tx, init_rx) = channel::<InitReport>();
        let (frame_tx, frame_rx) = channel::<EncodedUnit>();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = stop.clone();
        let encode_fps = Arc::new(AtomicU32::new(0));
        let encode_fps_thread = encode_fps.clone();
        let encode_kbps = Arc::new(AtomicU32::new(0));
        let encode_kbps_thread = encode_kbps.clone();
        let bitrate = Arc::new(AtomicU32::new(0));
        let bitrate_thread = bitrate.clone();
        let keyframe_thread = keyframe.clone();

        let handle = std::thread::Builder::new()
            .name("video-publisher".into())
            .spawn(move || {
                if let Err(e) = run(
                    &stop_thread,
                    fps,
                    window,
                    preset,
                    &frame_tx,
                    &init_tx,
                    &encode_fps_thread,
                    &encode_kbps_thread,
                    &bitrate_thread,
                    &keyframe_thread,
                ) {
                    eprintln!("[ВИДЕО] Публикация остановлена: {e}");
                }
            })
            .map_err(|e| format!("поток публикации: {e}"))?;

        // ждём инициализации захвата и энкодера, иначе нечего показывать
        match init_rx.recv() {
            Ok(Ok((info, source, base, used))) => Ok(Self {
                stop,
                handle: Some(handle),
                rx: frame_rx,
                info,
                source,
                encode_fps,
                encode_kbps,
                bitrate,
                bitrate_base: base,
                preset: used,
                keyframe,
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

#[allow(clippy::too_many_arguments)]
fn run(
    stop: &AtomicBool,
    fps: u32,
    window: Option<isize>,
    preset: Option<VideoPreset>,
    tx: &Sender<EncodedUnit>,
    init: &Sender<InitReport>,
    encode_fps: &AtomicU32,
    encode_kbps: &AtomicU32,
    bitrate: &AtomicU32,
    keyframe: &AtomicBool,
) -> Result<(), String> {
    // Окно — через WGC, если система умеет; иначе вырезка из кадра экрана.
    // Целимся в размер источника: апскейл тратит битрейт зря.
    let (mut capture, w, h, label) = match make_capture(fps, window) {
        Ok(v) => v,
        Err(e) => {
            let _ = init.send(Err(e.clone()));
            return Err(e);
        }
    };
    let used = preset.unwrap_or_else(|| VideoPreset::default_for(w, h));
    let target = VideoTarget::new(w, h, fps, used);
    let mut encoder = match MfEncoder::new(&target) {
        Ok(e) => e,
        Err(e) => {
            let _ = init.send(Err(e.clone()));
            return Err(e);
        }
    };
    // Пресет и битрейт — в журнал: по нему видно, с каким качеством идёт
    // поток и не «съел» ли его адаптер.
    println!(
        "[ВИДЕО] {} → {}x{} @ {} FPS · пресет {} · {:.1} Мбит/с · MFT {}",
        label,
        target.width,
        target.height,
        target.fps,
        used.label(),
        target.bitrate_bps as f32 / 1_000_000.0,
        encoder.mft_name
    );
    if !encoder.notes.is_empty() {
        println!("[ВИДЕО] ICodecAPI: {:?}", encoder.notes);
    }

    let info = StreamInfo {
        width: target.width,
        height: target.height,
        fps: target.fps,
        sequence_header: encoder.sequence_header.clone(),
    };
    let source = format!("{label} → {}x{}", target.width, target.height);
    let base_bitrate = target.bitrate_bps;
    let _ = init.send(Ok((info, source, base_bitrate, used)));
    // Какой битрейт сейчас стоит у энкодера: меняем только при отличии.
    let mut applied_bitrate = base_bitrate;

    let period = Duration::from_secs_f64(1.0 / fps.max(1) as f64);
    let mut next = Instant::now();
    // фактическая скорость кодирования: нужно понять, упирается ли пайплайн
    // в целевой fps или в захват/энкодер
    let mut fps_window = Instant::now();
    let mut fps_frames = 0u32;
    // сколько байт ушло в сеть за окно: битрейт важнее целевого, потому что
    // он и определяет, перегружен ли канал
    let mut window_bytes: u64 = 0;
    // Раз в 5 с печатаем фактические числа: по ним видно, упирается ли поток
    // в захват/энкодер, и сколько реально уходит в сеть.
    let mut log_windows = 0u32;
    let mut keyframe_warned = false;

    while !stop.load(Ordering::Relaxed) {
        let frame = match capture.next_nv12(50) {
            Ok(Some(buf)) => buf,
            Ok(None) => continue, // экран не менялся
            Err(e) => return Err(e),
        };
        // Адаптация канала: `VideoLink` пишет сюда новый битрейт, энкодер
        // применяет его без перезапуска потока. 0 — «не трогать».
        let wanted = bitrate.load(Ordering::Relaxed);
        if wanted != 0 && wanted != applied_bitrate {
            let previous = applied_bitrate;
            if encoder.set_mean_bitrate(wanted) {
                applied_bitrate = wanted;
                println!(
                    "[ВИДЕО] Битрейт {:.1} → {:.1} Мбит/с (потолок {:.1})",
                    previous as f32 / 1_000_000.0,
                    wanted as f32 / 1_000_000.0,
                    base_bitrate as f32 / 1_000_000.0
                );
            }
        }
        // Вошедшему зрителю нужен ключевой кадр сразу, а не через секунду:
        // `VideoLink` ставит флаг при подписке.
        if keyframe.swap(false, Ordering::Relaxed) && !encoder.force_keyframe() && !keyframe_warned {
            keyframe_warned = true;
            println!("[ВИДЕО] Энкодер не принял запрос ключевого кадра — зритель ждёт планового IDR");
        }
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
            let captured = (fps_frames as f32 / secs).round() as u32;
            encode_fps.store(captured, Ordering::Relaxed);
            // килобиты в секунду: байты × 8 / 1000
            let kbps = (window_bytes as f32 * 8.0 / 1000.0 / secs).round() as u32;
            encode_kbps.store(kbps, Ordering::Relaxed);
            fps_window = Instant::now();
            fps_frames = 0;
            window_bytes = 0;
            log_windows += 1;
            if log_windows.is_multiple_of(5) {
                println!(
                    "[ВИДЕО] захват {captured} FPS · {kbps} кбит/с · цель {:.1} Мбит/с",
                    applied_bitrate as f32 / 1_000_000.0
                );
            }
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


