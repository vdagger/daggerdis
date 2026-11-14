//! Headless-проверки видеотракта.
//!
//! Режимы:
//!   (без флагов) [w h fps frames] — синтетический NV12 в памяти → энкодер
//!   --gpu      [w h fps frames]   — BGRA-текстура → GPU-конверсия NV12 → энкодер
//!   --capture  [секунды]          — захват реального экрана (нужен рабочий стол!)
//!
//! Запуск из корня репозитория:
//!   cargo run --release --manifest-path tools/video_probe/Cargo.toml -- --gpu

#[cfg(not(windows))]
fn main() {
    println!("Пробник доступен только на Windows");
}

#[cfg(windows)]
fn main() {
    use albus::video::{win_mf::MfEncoder, VideoPreset, VideoTarget};

    let mode = std::env::args().nth(1).unwrap_or_default();
    let rest: Vec<String> = std::env::args().skip(2).collect();
    let nums: Vec<u32> = rest.iter().filter_map(|a| a.parse().ok()).collect();

    match mode.as_str() {
        "--capture" => run_capture(nums.first().copied().unwrap_or(5) as u64),
        "--gpu" => run_gpu(
            nums.first().copied().unwrap_or(2560),
            nums.get(1).copied().unwrap_or(1440),
            nums.get(2).copied().unwrap_or(60),
            nums.get(3).copied().unwrap_or(300) as usize,
        ),
        _ => {
            // без флага первый аргумент — ширина
            let all: Vec<String> = std::env::args().skip(1).collect();
            let nums: Vec<u32> = all.iter().filter_map(|a| a.parse().ok()).collect();
            run_cpu(
                nums.first().copied().unwrap_or(2560),
                nums.get(1).copied().unwrap_or(1440),
                nums.get(2).copied().unwrap_or(60),
                nums.get(3).copied().unwrap_or(180) as usize,
            );
        }
    }

    fn target_for(w: u32, h: u32, fps: u32) -> VideoTarget {
        let preset = VideoPreset::default_for(w, h);
        let t = VideoTarget::new(w, h, fps, preset);
        println!(
            "Цель: {}x{}@{} ({}) = {:.1} Мбит/с",
            t.width,
            t.height,
            t.fps,
            preset.label(),
            t.bitrate_bps as f64 / 1e6
        );
        t
    }

    fn new_encoder(target: &VideoTarget) -> MfEncoder {
        match MfEncoder::new(target) {
            Ok(enc) => {
                println!("MFT: {}", enc.mft_name);
                if !enc.notes.is_empty() {
                    println!("ICodecAPI: {:?}", enc.notes);
                }
                println!("SPS/PPS: {} байт", enc.sequence_header.len());
                enc
            }
            Err(e) => {
                println!("ОШИБКА энкодера: {e}");
                std::process::exit(1);
            }
        }
    }

    fn report(units: &mut usize, keyframes: &mut usize, out: Vec<albus::video::EncodedUnit>) {
        for u in out {
            *units += 1;
            if u.keyframe {
                *keyframes += 1;
            }
        }
    }

    fn run_cpu(width: u32, height: u32, fps: u32, frames: usize) {
        use std::time::Instant;
        let target = target_for(width, height, fps);
        for line in albus::video::win_mf::diagnose(&target) {
            println!("  {line}");
        }
        let mut enc = new_encoder(&target);

        let mut nv12 = vec![0u8; (target.width * target.height * 3 / 2) as usize];
        let mut total = 0usize;
        let (mut n, mut keys) = (0usize, 0usize);

        let start = Instant::now();
        for i in 0..frames {
            fill_nv12(&mut nv12, target.width, target.height, i as u32);
            let units = match enc.push_nv12(&nv12) {
                Ok(u) => u,
                Err(e) => {
                    println!("ОШИБКА на кадре {i}: {e}");
                    std::process::exit(1);
                }
            };
            for u in &units {
                total += u.data.len();
            }
            report(&mut n, &mut keys, units);
        }
        if let Ok(units) = enc.finish() {
            for u in &units {
                total += u.data.len();
            }
            report(&mut n, &mut keys, units);
        }
        let secs = start.elapsed().as_secs_f64();
        let video_secs = frames as f64 / fps as f64;
        println!("Закодировано: {} кадров за {:.2} с ({:.1} fps)", n, secs, n as f64 / secs);
        println!(
            "Байт: {} ({:.2} Мбит/с по видео), ключевых: {}",
            total,
            total as f64 * 8.0 / video_secs / 1e6,
            keys
        );
    }

    fn run_gpu(width: u32, height: u32, fps: u32, frames: usize) {
        use albus::video::win_d3d;
        use std::time::Instant;

        let target = target_for(width, height, fps);
        let d3d = match win_d3d::create_device(None) {
            Ok(d) => d,
            Err(e) => {
                println!("ОШИБКА D3D11: {e}");
                std::process::exit(1);
            }
        };
        let tex = match win_d3d::bgra_texture(&d3d.device, target.width, target.height) {
            Ok(t) => t,
            Err(e) => {
                println!("ОШИБКА текстуры: {e}");
                std::process::exit(1);
            }
        };
        let mut conv = match win_d3d::Converter::new(
            &d3d.device,
            &d3d.context,
            win_d3d::FORMAT_BGRA,
            target.width,
            target.height,
            win_d3d::FORMAT_NV12,
            target.width,
            target.height,
            target.fps,
        ) {
            Ok(c) => c,
            Err(e) => {
                println!("ОШИБКА видеопроцессора: {e}");
                std::process::exit(1);
            }
        };
        let mut enc = new_encoder(&target);

        let mut bgra = vec![0u8; (target.width * target.height * 4) as usize];
        let mut total = 0usize;
        let (mut n, mut keys) = (0usize, 0usize);

        let start = Instant::now();
        for i in 0..frames {
            fill_bgra(&mut bgra, target.width, target.height, i as u32);
            win_d3d::upload_bgra(&d3d.context, &tex, target.width, &bgra);
            let nv12 = match conv.convert_and_read(&tex) {
                Ok(v) => v,
                Err(e) => {
                    println!("ОШИБКА конверсии на кадре {i}: {e}");
                    std::process::exit(1);
                }
            };
            let units = match enc.push_nv12(&nv12) {
                Ok(u) => u,
                Err(e) => {
                    println!("ОШИБКА энкодера на кадре {i}: {e}");
                    std::process::exit(1);
                }
            };
            for u in &units {
                total += u.data.len();
            }
            report(&mut n, &mut keys, units);
        }
        if let Ok(units) = enc.finish() {
            for u in &units {
                total += u.data.len();
            }
            report(&mut n, &mut keys, units);
        }
        let secs = start.elapsed().as_secs_f64();
        let video_secs = frames as f64 / fps as f64;
        println!(
            "GPU-конвейер: {} кадров за {:.2} с ({:.1} fps)",
            n,
            secs,
            n as f64 / secs
        );
        println!(
            "Байт: {} ({:.2} Мбит/с по видео), ключевых: {}",
            total,
            total as f64 * 8.0 / video_secs / 1e6,
            keys
        );
    }

    fn run_capture(seconds: u64) {
        use albus::video::win_dxgi::ScreenCapture;
        use std::time::{Duration, Instant};

        let mut cap = match ScreenCapture::new(None, 60) {
            Ok(c) => c,
            Err(e) => {
                println!("ОШИБКА захвата: {e}");
                println!("Захват работает только в интерактивном сеансе пользователя.");
                std::process::exit(1);
            }
        };
        let (out_w, out_h) = cap.dimensions();
        println!(
            "Экран: {} {}x{} → {}x{}",
            cap.adapter_name, cap.source_width, cap.source_height, out_w, out_h
        );

        let deadline = Instant::now() + Duration::from_secs(seconds);
        let start = Instant::now();
        let mut frames = 0u64;
        let mut bytes = 0u64;
        let mut last_y = 0u64;
        while Instant::now() < deadline {
            match cap.next_nv12(100) {
                Ok(Some(buf)) => {
                    frames += 1;
                    bytes += buf.len() as u64;
                    let n = (out_w * out_h) as usize;
                    last_y = buf[..n.min(buf.len())]
                        .iter()
                        .map(|&v| v as u64)
                        .sum::<u64>()
                        / n.max(1) as u64;
                }
                Ok(None) => {}
                Err(e) => {
                    println!("ОШИБКА кадра: {e}");
                    break;
                }
            }
        }
        let secs = start.elapsed().as_secs_f64();
        println!(
            "Захвачено: {} кадров за {:.2} с ({:.1} fps), средняя яркость Y={}",
            frames,
            secs,
            frames as f64 / secs,
            last_y
        );
        println!("Байт NV12 на кадр: {}", bytes / frames.max(1));
    }

    fn fill_nv12(buf: &mut [u8], w: u32, h: u32, frame: u32) {
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

    fn fill_bgra(buf: &mut [u8], w: u32, h: u32, frame: u32) {
        let bar = (frame * 8) % w;
        for y in 0..h as usize {
            for x in 0..w as usize {
                let i = (y * w as usize + x) * 4;
                let grad = ((x * 255 / w as usize) + (y * 255 / h as usize)) / 2;
                let moving = if (x as i64 - bar as i64).abs() < 40 {
                    235
                } else {
                    0
                };
                let v = (grad as u8).saturating_add(moving);
                buf[i] = v;
                buf[i + 1] = v;
                buf[i + 2] = v;
                buf[i + 3] = 255;
            }
        }
    }
}
