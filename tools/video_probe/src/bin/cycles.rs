//! Циклы «старт/стоп» без GUI.
//!
//! Каждый круг создаёт и бросает ровно то, что живёт между нажатиями
//! «Начать трансляцию» и «Остановить»: H.264-энкодер, декодер, D3D-устройство
//! с текстурами превью. После каждого круга печатается занятая память — так
//! видно, течёт ли что-то на повторных трансляциях, без кликов в приложении.
//!
//! Запуск: cycles [кругов] [ширина высота fps кадров] [--enc|--dec|--gpu]
//!   cycles                — 6 кругов, 1920x1080@60, 60 кадров в круге
//!   cycles 10 2560 1440 60 60
//!   cycles 6 1920 1080 60 60 --enc   — только энкодер (так ищем утечку)

#[cfg(not(windows))]
fn main() {
    println!("Пробник доступен только на Windows");
}

#[cfg(windows)]
fn main() {
    use albus::video::publisher::StreamInfo;
    use albus::video::win_d3d::{self, Converter, FORMAT_BGRA, FORMAT_NV12};
    use albus::video::win_decode::MfDecoder;
    use albus::video::win_mf::MfEncoder;
    use albus::video::{EncodedUnit, VideoPreset, VideoTarget};
    use std::time::{Duration, Instant};

    let nums: Vec<u32> = std::env::args()
        .skip(1)
        .filter_map(|a| a.parse().ok())
        .collect();
    let cycles = nums.first().copied().unwrap_or(6) as usize;
    let w = nums.get(1).copied().unwrap_or(1920);
    let h = nums.get(2).copied().unwrap_or(1080);
    let fps = nums.get(3).copied().unwrap_or(60);
    let frames = nums.get(4).copied().unwrap_or(60) as usize;

    // --enc / --dec / --gpu оставляют в круге только одну часть пайплайна:
    // так видно, какой компонент не отпускает память
    let flag = |f: &str| std::env::args().any(|a| a == f);
    let (only_enc, only_dec, only_gpu) = (flag("--enc"), flag("--dec"), flag("--gpu"));
    // ступени GPU-пути: видно, какой объект не отпускает память
    let gpu_step = if flag("--dev") {
        1
    } else if flag("--tex") {
        2
    } else if flag("--conv") {
        3
    } else {
        4
    };
    // что делать с каждым кадром: 1 — только заливка, 2 — +конверсия,
    // 3 — +копия в staging, 4 — +чтение в память
    let work = if flag("--upload") {
        1
    } else if flag("--blt") {
        2
    } else if flag("--copy") {
        3
    } else {
        4
    };
    let only_gpu = only_gpu || gpu_step < 4 || work < 4;
    let only_one = only_enc || only_dec || only_gpu;

    println!(
        "кругов: {cycles}, {w}x{h}@{fps}, кадров в круге: {frames}, режим: {}, pid {}",
        if only_enc {
            "только энкодер"
        } else if only_dec {
            "только декодер"
        } else if only_gpu {
            "только GPU"
        } else {
            "весь пайплайн"
        },
        std::process::id()
    );
    println!("старт: {}", fmt(mem()));

    // для режима --dec: поток кодируем один раз, дальше вкруговую только декодер
    let mut stream: Vec<EncodedUnit> = Vec::new();
    let mut stream_info: Option<StreamInfo> = None;
    if only_dec {
        let target = VideoTarget::new(w, h, fps, VideoPreset::default_for(w, h));
        let mut enc = match MfEncoder::new(&target) {
            Ok(e) => e,
            Err(e) => {
                println!("ОШИБКА энкодера: {e}");
                return;
            }
        };
        let mut nv12 = vec![0u8; (target.width * target.height * 3 / 2) as usize];
        for i in 0..frames {
            fill_nv12(&mut nv12, target.width, target.height, i as u32);
            if let Ok(units) = enc.push_nv12(&nv12) {
                stream.extend(units);
            }
        }
        if let Ok(units) = enc.finish() {
            stream.extend(units);
        }
        stream_info = Some(StreamInfo {
            width: target.width,
            height: target.height,
            fps: target.fps,
            sequence_header: enc.sequence_header.clone(),
        });
        drop(enc);
        std::thread::sleep(Duration::from_millis(1500));
        println!("поток готов: {} кадров, {}", stream.len(), fmt(mem()));
    }

    for cycle in 0..cycles {
        let t = Instant::now();
        let target = VideoTarget::new(w, h, fps, VideoPreset::default_for(w, h));
        let mut decoded = 0usize;
        let mut mft = String::new();

        if only_dec {
            let mut dec = match MfDecoder::new(stream_info.as_ref().unwrap()) {
                Ok(d) => d,
                Err(e) => {
                    println!("ОШИБКА декодера: {e}");
                    return;
                }
            };
            mft = dec.mft_name.clone();
            for u in &stream {
                if let Ok(frames) = dec.push(u) {
                    if frames.into_iter().last().is_some() {
                        decoded += 1;
                    }
                }
            }
            drop(dec);
        }

        if only_gpu {
            let d3d = match win_d3d::create_device(None) {
                Ok(d) => d,
                Err(e) => {
                    println!("ОШИБКА D3D11: {e}");
                    return;
                }
            };
            if gpu_step >= 2 {
                let nv12_tex =
                    match win_d3d::nv12_texture(&d3d.device, target.width, target.height) {
                        Ok(t) => t,
                        Err(e) => {
                            println!("ОШИБКА текстуры NV12: {e}");
                            return;
                        }
                    };
                if gpu_step >= 3 {
                    let (out_w, out_h) = ((target.width / 2) & !1, (target.height / 2) & !1);
                    let mut conv = match Converter::new(
                        &d3d.device,
                        &d3d.context,
                        FORMAT_NV12,
                        target.width,
                        target.height,
                        FORMAT_BGRA,
                        out_w,
                        out_h,
                        target.fps,
                    ) {
                        Ok(c) => c,
                        Err(e) => {
                            println!("ОШИБКА видеопроцессора: {e}");
                            return;
                        }
                    };
                    if gpu_step >= 4 {
                        let nv12 = vec![128u8; (target.width * target.height * 3 / 2) as usize];
                        for _ in 0..frames {
                            win_d3d::upload_nv12(&d3d.context, &nv12_tex, target.width, &nv12);
                            if work >= 2 && conv.convert(&nv12_tex).is_err() {
                                break;
                            }
                            if work >= 3 && conv.copy_to_staging().is_err() {
                                break;
                            }
                            if work >= 4 && conv.read_back().is_err() {
                                break;
                            }
                            decoded += 1;
                        }
                    }
                    drop(conv);
                }
                drop(nv12_tex);
            }
            drop(d3d);
        }

        if !only_one || only_enc {
            let mut enc = match MfEncoder::new(&target) {
                Ok(e) => e,
                Err(e) => {
                    println!("ОШИБКА энкодера: {e}");
                    return;
                }
            };
            let info = StreamInfo {
                width: target.width,
                height: target.height,
                fps: target.fps,
                sequence_header: enc.sequence_header.clone(),
            };

            let mut dec: Option<MfDecoder> = if only_enc {
                mft = enc.mft_name.clone();
                None
            } else {
                match MfDecoder::new(&info) {
                    Ok(d) => {
                        mft = format!("{} / {}", enc.mft_name, d.mft_name);
                        Some(d)
                    }
                    Err(e) => {
                        println!("ОШИБКА декодера: {e}");
                        return;
                    }
                }
            };

            // превью держит D3D-устройство, NV12-текстуру и видеопроцессор —
            // ровно набор `Renderer`. Типы windows-крейта здесь не называем:
            // замыкание захватывает их само.
            let mut show: Option<Box<dyn FnMut(&[u8])>> = if only_enc {
                None
            } else {
                let d3d = match win_d3d::create_device(None) {
                    Ok(d) => d,
                    Err(e) => {
                        println!("ОШИБКА D3D11: {e}");
                        return;
                    }
                };
                let tex = match win_d3d::nv12_texture(&d3d.device, target.width, target.height) {
                    Ok(t) => t,
                    Err(e) => {
                        println!("ОШИБКА текстуры NV12: {e}");
                        return;
                    }
                };
                let (out_w, out_h) = ((target.width / 2) & !1, (target.height / 2) & !1);
                let mut conv = match Converter::new(
                    &d3d.device,
                    &d3d.context,
                    FORMAT_NV12,
                    target.width,
                    target.height,
                    FORMAT_BGRA,
                    out_w,
                    out_h,
                    target.fps,
                ) {
                    Ok(c) => c,
                    Err(e) => {
                        println!("ОШИБКА видеопроцессора: {e}");
                        return;
                    }
                };
                let tw = target.width;
                Some(Box::new(move |f: &[u8]| {
                    win_d3d::upload_nv12(&d3d.context, &tex, tw, f);
                    let _ = conv.convert(&tex);
                    let _ = conv.copy_to_staging();
                    let _ = conv.read_back();
                }))
            };

            let mut nv12 = vec![0u8; (target.width * target.height * 3 / 2) as usize];
            for i in 0..frames {
                fill_nv12(&mut nv12, target.width, target.height, i as u32);
                let Ok(units) = enc.push_nv12(&nv12) else {
                    break;
                };
                match &mut dec {
                    None => decoded += units.len(),
                    Some(d) => {
                        for u in &units {
                            let Ok(frames) = d.push(u) else { continue };
                            if let Some(f) = frames.into_iter().last() {
                                if f.len() >= nv12.len() {
                                    if let Some(show) = &mut show {
                                        show(&f);
                                    }
                                    decoded += 1;
                                }
                            }
                        }
                    }
                }
            }
            let _ = enc.finish();

            // порядок как в приложении: сначала превью, потом энкодер
            drop(show);
            drop(dec);
            drop(enc);
        }

        // драйверы освобождают GPU-ресурсы не мгновенно
        std::thread::sleep(Duration::from_millis(1500));

        println!(
            "круг {}: {} ({} кадров, {:.1} с, {mft})",
            cycle + 1,
            fmt(mem()),
            decoded,
            t.elapsed().as_secs_f32()
        );
    }
}

/// Занятая процессом память: зафиксировано (commit) и рабочее множество.
#[cfg(windows)]
fn mem() -> (usize, usize) {
    #[repr(C)]
    #[derive(Default)]
    struct ProcessMemoryCounters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn K32GetProcessMemoryInfo(
            process: isize,
            counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
        fn GetCurrentProcess() -> isize;
    }
    unsafe {
        let mut c = ProcessMemoryCounters {
            cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
            ..Default::default()
        };
        if K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) == 0 {
            return (0, 0);
        }
        (c.pagefile_usage, c.working_set_size)
    }
}

#[cfg(windows)]
fn fmt((commit, ws): (usize, usize)) -> String {
    format!(
        "commit {:.0} МБ, рабочее множество {:.0} МБ",
        commit as f64 / (1024.0 * 1024.0),
        ws as f64 / (1024.0 * 1024.0)
    )
}

#[cfg(windows)]
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
