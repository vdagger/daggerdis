//! Спайк: проверка аппаратного H.264-энкодера без GUI и без захвата.
//!
//! Синтетическая картинка в NV12 → Media Foundation MFT → статистика.
//! Запуск из корня репозитория:
//!   cargo run --release --manifest-path tools/video_probe/Cargo.toml -- [ширина высота fps кадров]

#[cfg(not(windows))]
fn main() {
    println!("Спайк доступен только на Windows");
}

#[cfg(windows)]
fn main() {
    use albus::video::{win_mf::MfEncoder, VideoPreset, VideoTarget};
    use std::time::Instant;

    let args: Vec<u32> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let width = args.first().copied().unwrap_or(2560);
    let height = args.get(1).copied().unwrap_or(1440);
    let fps = args.get(2).copied().unwrap_or(60);
    let frames = args.get(3).copied().unwrap_or(180) as usize;

    let preset = VideoPreset::default_for(width, height);
    let target = VideoTarget::new(width, height, fps, preset);
    println!(
        "Цель: {}x{}@{} ({}) = {:.1} Мбит/с",
        target.width,
        target.height,
        target.fps,
        preset.label(),
        target.bitrate_bps as f64 / 1e6
    );

    match albus::video::win_mf::available_encoders() {
        Ok(list) => println!("Аппаратные энкодеры: {:?}", list),
        Err(e) => println!("Список энкодеров недоступен: {e}"),
    }
    for line in albus::video::win_mf::diagnose(&target) {
        println!("  {line}");
    }

    let mut enc = match MfEncoder::new(&target) {
        Ok(e) => e,
        Err(e) => {
            println!("ОШИБКА: {e}");
            std::process::exit(1);
        }
    };
    println!("MFT: {}", enc.mft_name);
    if !enc.notes.is_empty() {
        println!("ICodecAPI: {:?}", enc.notes);
    }
    println!("SPS/PPS: {} байт", enc.sequence_header.len());

    let mut nv12 = vec![0u8; (target.width * target.height * 3 / 2) as usize];
    let mut total = 0usize;
    let mut keyframes = 0usize;
    let mut first_pts: i64 = -1;

    let start = Instant::now();
    for i in 0..frames {
        fill_test_pattern(&mut nv12, target.width, target.height, i as u32);
        match enc.push_nv12(&nv12) {
            Ok(units) => {
                for u in units {
                    if first_pts < 0 {
                        first_pts = u.pts_100ns;
                    }
                    total += u.data.len();
                    if u.keyframe {
                        keyframes += 1;
                    }
                }
            }
            Err(e) => {
                println!("ОШИБКА на кадре {i}: {e}");
                std::process::exit(1);
            }
        }
    }
    if let Ok(units) = enc.finish() {
        for u in units {
            total += u.data.len();
            if u.keyframe {
                keyframes += 1;
            }
        }
    }

    let secs = start.elapsed().as_secs_f64();
    println!(
        "Закодировано: {} кадров за {:.2} с ({:.1} fps)",
        frames,
        secs,
        frames as f64 / secs
    );
    // битрейт считаем по длительности видео, а не по времени кодирования
    let video_secs = frames as f64 / fps as f64;
    println!(
        "Байт: {} ({:.2} Мбит/с в среднем по видео)",
        total,
        total as f64 * 8.0 / video_secs / 1e6
    );
    println!("Ключевых кадров: {}", keyframes);
    println!("Первый pts: {} (100 нс)", first_pts);
}

#[cfg(windows)]
fn fill_test_pattern(buf: &mut [u8], w: u32, h: u32, frame: u32) {
    let y_plane = (w * h) as usize;
    let bar = (frame * 8) % w;
    for y in 0..h as usize {
        let row = y * w as usize;
        for x in 0..w as usize {
            let grad = ((x * 255 / w as usize) + (y * 255 / h as usize)) / 2;
            let moving = if (x as i64 - bar as i64).abs() < 40 {
                235
            } else {
                0
            };
            buf[row + x] = (grad as u8).saturating_add(moving);
        }
    }
    for b in &mut buf[y_plane..] {
        *b = 128;
    }
}
