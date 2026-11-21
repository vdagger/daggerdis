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
        "--loopback" => run_loopback(
            nums.first().copied().unwrap_or(1920),
            nums.get(1).copied().unwrap_or(1080),
            nums.get(2).copied().unwrap_or(60),
            nums.get(3).copied().unwrap_or(180) as usize,
        ),
        "--roundtrip" => run_roundtrip(
            nums.first().copied().unwrap_or(1108),
            nums.get(1).copied().unwrap_or(610),
            nums.get(2).copied().unwrap_or(60),
            nums.get(3).copied().unwrap_or(30) as usize,
        ),
        // --window <hwnd> <секунды> <префикс>: захват окна через WGC и два
        // дампа NV12 — что сняли и что декодировали после кодека. Картинки
        // сравниваются глазами (или PIL), это единственный способ увидеть
        // «диагональные полосы» без GUI.
        "--window" => run_window(
            rest.first().and_then(|s| s.parse().ok()).unwrap_or(0),
            nums.get(1).copied().unwrap_or(3) as u64,
            rest.get(2).cloned().unwrap_or_else(|| "window".into()),
            nums.get(2).copied().unwrap_or(60),
        ),
        // --desktop <секунды> <префикс>: захват звука рабочего стола (process
        // loopback с исключением своего процесса). Пишет сырой f32 стерео.
        "--desktop" => run_desktop(
            nums.first().copied().unwrap_or(3) as u64,
            rest.get(1).cloned().unwrap_or_else(|| "desktop".into()),
        ),
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

    /// Энкодер → декодер на неподвижной картинке. Проверяем не скорость, а
    /// **сам кадр**: у декодера ширина строки выровнена (1108 → 1120), и если
    /// читать буфер как плотный, картинка уезжает диагональными полосами. Это
    /// видно только на размерах с шириной не кратной 16 — ровно как окно
    /// браузера 1108x610.
    fn run_roundtrip(width: u32, height: u32, fps: u32, frames: usize) {
        use albus::video::publisher::StreamInfo;
        use albus::video::win_decode::MfDecoder;

        let target = target_for(width, height, fps);
        let mut enc = new_encoder(&target);
        let info = StreamInfo {
            width: target.width,
            height: target.height,
            fps: target.fps,
            sequence_header: enc.sequence_header.clone(),
        };
        let mut dec = match MfDecoder::new(&info) {
            Ok(d) => d,
            Err(e) => {
                println!("ОШИБКА декодера: {e}");
                std::process::exit(1);
            }
        };
        println!(
            "Декодер: {} {:?} · шаг строки {:?}",
            dec.mft_name,
            dec.notes,
            dec.output_stride()
        );

        let mut nv12 = vec![0u8; (target.width * target.height * 3 / 2) as usize];
        fill_nv12(&mut nv12, target.width, target.height, 0);
        let mut decoded: Option<Vec<u8>> = None;
        for _ in 0..frames {
            let units = match enc.push_nv12(&nv12) {
                Ok(u) => u,
                Err(e) => {
                    println!("ОШИБКА энкодера: {e}");
                    std::process::exit(1);
                }
            };
            for u in units {
                match dec.push(&u) {
                    Ok(fs) => {
                        if let Some(f) = fs.into_iter().last() {
                            decoded = Some(f);
                        }
                    }
                    Err(e) => {
                        println!("ОШИБКА декодера: {e}");
                        std::process::exit(1);
                    }
                }
            }
        }
        if let Ok(units) = enc.finish() {
            for u in units {
                if let Ok(fs) = dec.push(&u) {
                    if let Some(f) = fs.into_iter().last() {
                        decoded = Some(f);
                    }
                }
            }
        }
        let Some(frame) = decoded else {
            println!("ОШИБКА: декодер не отдал ни одного кадра");
            std::process::exit(1);
        };
        let pixels = (target.width * target.height) as usize;
        let need = pixels * 3 / 2;
        let mad = if frame.len() >= pixels {
            let sum: u64 = frame[..pixels]
                .iter()
                .zip(&nv12[..pixels])
                .map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs() as u64)
                .sum();
            sum / pixels as u64
        } else {
            u64::MAX
        };
        println!(
            "Кадр: {} байт (плотный {}), MAD по Y = {} {}",
            frame.len(),
            need,
            mad,
            if mad <= 6 { "— картинка сходится" } else { "— КАДР ИСПОРЧЕН" }
        );
    }

    /// Захват окна через WGC → NV12, затем тот же NV12 через энкодер и
    /// декодер. Пишем два сырых файла `.cap.nv12` и `.dec.nv12`: их можно
    /// посмотреть PIL-ом и увидеть, где именно портится картинка.
    fn run_window(hwnd: isize, seconds: u64, prefix: String, fps: u32) {
        use albus::video::publisher::StreamInfo;
        use albus::video::win_decode::MfDecoder;
        use albus::video::win_wgc::WindowCapture;
        use std::time::{Duration, Instant};

        if hwnd == 0 {
            println!("ОШИБКА: нужен hwnd окна: --window <hwnd> <секунды> <префикс>");
            std::process::exit(1);
        }
        let mut cap = match WindowCapture::new(hwnd, fps) {
            Ok(c) => c,
            Err(e) => {
                println!("ОШИБКА WGC: {e}");
                std::process::exit(1);
            }
        };
        let (w, h) = cap.dimensions();
        println!("Окно: {w}x{h}");
        let deadline = Instant::now() + Duration::from_secs(seconds);
        let mut last: Option<Vec<u8>> = None;
        let mut frames = 0u32;
        while Instant::now() < deadline {
            match cap.next_nv12(100) {
                Ok(Some(buf)) => {
                    frames += 1;
                    last = Some(buf);
                }
                Ok(None) => {}
                Err(e) => {
                    println!("ОШИБКА кадра: {e}");
                    break;
                }
            }
        }
        let Some(cap_frame) = last else {
            println!("ОШИБКА: WGC не отдал ни кадра");
            std::process::exit(1);
        };
        println!("Снято кадров: {frames}, NV12 {} байт", cap_frame.len());
        let _ = std::fs::write(format!("{prefix}.cap.nv12"), &cap_frame);

        // Тот же кадр — через кодек.
        let target = target_for(w, h, fps);
        let mut enc = new_encoder(&target);
        let info = StreamInfo {
            width: target.width,
            height: target.height,
            fps: target.fps,
            sequence_header: enc.sequence_header.clone(),
        };
        let mut dec = match MfDecoder::new(&info) {
            Ok(d) => d,
            Err(e) => {
                println!("ОШИБКА декодера: {e}");
                std::process::exit(1);
            }
        };
        let mut decoded: Option<Vec<u8>> = None;
        for _ in 0..10 {
            if let Ok(units) = enc.push_nv12(&cap_frame) {
                for u in units {
                    if let Ok(fs) = dec.push(&u) {
                        if let Some(f) = fs.into_iter().last() {
                            decoded = Some(f);
                        }
                    }
                }
            }
        }
        if let Ok(units) = enc.finish() {
            for u in units {
                if let Ok(fs) = dec.push(&u) {
                    if let Some(f) = fs.into_iter().last() {
                        decoded = Some(f);
                    }
                }
            }
        }
        match decoded {
            Some(frame) => {
                println!("Декодировано {} байт", frame.len());
                let _ = std::fs::write(format!("{prefix}.dec.nv12"), &frame);
                // Путь показа: NV12 → текстура → GPU-конверсия в BGRA.
                // Превью своей трансляции идёт в половинном размере, поэтому
                // проверяем оба — именно этот шаг не покрыт ни fps-замерами,
                // ни дампом декодированного кадра.
                dump_bgra(w, h, &frame, &prefix);
            }
            None => println!("ОШИБКА: декодер не отдал кадр"),
        }
    }

    /// Декодированный NV12 → текстура → конверсия в BGRA (как в показе) и
    /// дамп в файл: полный размер и половинный, как у превью.
    fn dump_bgra(w: u32, h: u32, nv12: &[u8], prefix: &str) {
        use albus::video::win_d3d::{self, FORMAT_BGRA, FORMAT_NV12};
        let d3d = match win_d3d::create_device(None) {
            Ok(d) => d,
            Err(e) => {
                println!("ОШИБКА D3D11: {e}");
                return;
            }
        };
        let tex = match win_d3d::nv12_texture(&d3d.device, w, h) {
            Ok(t) => t,
            Err(e) => {
                println!("ОШИБКА текстуры: {e}");
                return;
            }
        };
        win_d3d::upload_nv12(&d3d.context, &tex, w, nv12);
        for (tag, ow, oh) in [
            ("full", w & !1, h & !1),
            ("half", (w / 2) & !1, (h / 2) & !1),
        ] {
            let mut conv = match win_d3d::Converter::new(
                &d3d.device,
                &d3d.context,
                FORMAT_NV12,
                w,
                h,
                FORMAT_BGRA,
                ow,
                oh,
                60,
            ) {
                Ok(c) => c,
                Err(e) => {
                    println!("ОШИБКА конвертера {ow}x{oh}: {e}");
                    continue;
                }
            };
            if let Err(e) = conv
                .convert(&tex)
                .and_then(|_| conv.copy_to_staging())
                .and_then(|_| conv.read_back())
                .map(|bgra| {
                    let path = format!("{prefix}.{tag}.bgra");
                    let _ = std::fs::write(&path, &bgra);
                    println!("{tag}: BGRA {ow}x{oh} → {path} ({} байт)", bgra.len());
                })
            {
                println!("ОШИБКА показа {ow}x{oh}: {e}");
            }
        }
    }

    /// Захват звука рабочего стола: проверка, что process loopback вообще
    /// поднимается и что в него попадает чужой звук (мы сами исключены).
    fn run_desktop(seconds: u64, prefix: String) {
        use albus::desktop_audio::{frame_level, DesktopCapture};
        use std::time::{Duration, Instant};

        let cap = match DesktopCapture::start() {
            Ok(c) => c,
            Err(e) => {
                println!("ОШИБКА захвата звука стола: {e}");
                std::process::exit(1);
            }
        };
        println!("Захват звука стола пошёл (свой процесс исключён)");
        let mut all: Vec<f32> = Vec::new();
        let (mut max_rms, mut max_peak, mut frames) = (0.0f32, 0.0f32, 0u32);
        let deadline = Instant::now() + Duration::from_secs(seconds);
        while Instant::now() < deadline {
            let frame = cap.read_frame();
            let (rms, peak) = frame_level(&frame);
            max_rms = max_rms.max(rms);
            max_peak = max_peak.max(peak);
            all.extend_from_slice(&frame);
            frames += 1;
            std::thread::sleep(Duration::from_millis(20));
        }
        let bytes: Vec<u8> = all.iter().flat_map(|s| s.to_le_bytes()).collect();
        let path = format!("{prefix}.f32");
        let _ = std::fs::write(&path, &bytes);
        println!(
            "Кадров: {frames}, сэмплов: {}, RMS макс {max_rms:.4}, пик {max_peak:.4}, недоборов {}",
            all.len(),
            cap.underruns()
        );
        println!("Записано: {path} ({} байт)", bytes.len());
    }

    /// Полный круг без экрана и сети: синтетика → H.264 → декодер.
    fn run_loopback(width: u32, height: u32, fps: u32, frames: usize) {        use albus::video::publisher::StreamInfo;
        use albus::video::win_decode::MfDecoder;
        use std::time::Instant;

        let target = target_for(width, height, fps);
        let mut enc = new_encoder(&target);
        let info = StreamInfo {
            width: target.width,
            height: target.height,
            fps: target.fps,
            sequence_header: enc.sequence_header.clone(),
        };
        let mut dec = match MfDecoder::new(&info) {
            Ok(d) => d,
            Err(e) => {
                println!("ОШИБКА декодера: {e}");
                std::process::exit(1);
            }
        };
        println!("Декодер: {} {:?}", dec.mft_name, dec.notes);

        let mut nv12 = vec![0u8; (target.width * target.height * 3 / 2) as usize];
        let (mut decoded, mut bytes) = (0usize, 0usize);
        let start = Instant::now();
        for i in 0..frames {
            fill_nv12(&mut nv12, target.width, target.height, i as u32);
            let units = match enc.push_nv12(&nv12) {
                Ok(u) => u,
                Err(e) => {
                    println!("ОШИБКА энкодера: {e}");
                    std::process::exit(1);
                }
            };
            for u in &units {
                match dec.push(u) {
                    Ok(frames) => {
                        for f in frames {
                            decoded += 1;
                            bytes += f.len();
                        }
                    }
                    Err(e) => {
                        println!("ОШИБКА декодера: {e}");
                        std::process::exit(1);
                    }
                }
            }
        }
        let secs = start.elapsed().as_secs_f64();
        println!(
            "Круговой прогон: {} кадров декодировано за {:.2} с ({:.1} fps), {} байт NV12 на кадр",
            decoded,
            secs,
            decoded as f64 / secs,
            bytes / decoded.max(1)
        );
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
