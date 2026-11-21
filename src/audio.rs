use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

pub struct UniversalAudioCapture;

/// Канал считается выбранным, только если он существует на карте.
/// На канал 0 при неверном индексе не откатываемся: иначе моно-микрофон
/// со старым «вторым каналом» попал бы в оба входа.
fn channel_usable(channel: u32, total_channels: usize) -> bool {
    channel != u32::MAX && (channel as usize) < total_channels
}

/// Программный ресэмплер методом линейной интерполяции
/// Преобразует аудиопоток из одной частоты дискретизации в другую без изменения тональности (питча)
fn resample_linear(input: &[f32], from_rate: u32, to_rate: u32, position: &mut f64) -> Vec<f32> {
    if from_rate == to_rate || input.is_empty() {
        return input.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let mut output = Vec::with_capacity((input.len() as f64 / ratio).ceil() as usize + 2);
    let last = input.len() - 1;

    // Позиция переносится на следующий буфер как смещение ОТ ЕГО НАЧАЛА.
    // Раньше вычиталось `len - 1`, и на каждой границе фаза съезжала на один
    // семпл: кусок сигнала терялся, а звук на каждом буфере получал тик
    // (чем меньше буфер, тем заметнее). За последним семплом держим его же.
    while *position < input.len() as f64 {
        let idx = (*position as usize).min(last);
        let frac = (*position - idx as f64) as f32;
        let s0 = input[idx];
        let s1 = input.get(idx + 1).copied().unwrap_or(s0);
        output.push(s0 + frac * (s1 - s0));
        *position += ratio;
    }

    *position -= input.len() as f64;
    output
}

/// Пакетная потеря кадра: декодер продолжает сигнал по модели.
///
/// В Opus пустой пакет для `decode` — это и есть PLC (`opus_decode` с
/// `data = NULL`): продолжение тона и формы спектра по истории, а не повтор
/// последнего кадра. Возвращает 0, если декодер ещё не слышал ни одного пакета.
fn plc(decoder: &mut rusty_opus::OpusDecoder, out: &mut [f32; 960]) -> usize {
    decoder.decode(&[], 960, out).unwrap_or(0)
}

impl UniversalAudioCapture {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(UniversalAudioCapture)
    }

    /// Открывает входной поток. `fill` говорит, какие слоты шины этот поток
    /// наполняет: `[true, true]` — одна карта на оба входа (как раньше),
    /// `[true, false]`/`[false, true]` — отдельное физическое устройство.
    pub fn create_input_stream(
        &mut self,
        device: Option<cpal::Device>,
        tx: crate::txbus::TxContext,
        fill: [bool; 2],
        epochs: [u64; 2],
    ) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let input_device = match device {
            Some(d) => d,
            None => host
                .default_input_device()
                .ok_or("No default input device")?,
        };

        // Кандидаты: 48 кГц (ресэмплер не нужен) в порядке f32 → i16 → i32,
        // затем родной формат устройства по умолчанию. Раньше поток всегда
        // строился как f32: у карты, которая на 48 кГц даёт только i16, он
        // не открывался вовсе — свой голос не уходил в эфир, хотя собеседник
        // всё делал правильно.
        let ranges: Vec<cpal::SupportedStreamConfigRange> = input_device
            .supported_input_configs()
            .map(|configs| configs.collect())
            .unwrap_or_default();

        let mut candidates: Vec<(cpal::SampleFormat, cpal::StreamConfig)> = Vec::new();
        for fmt in [
            cpal::SampleFormat::F32,
            cpal::SampleFormat::I16,
            cpal::SampleFormat::I32,
        ] {
            let range = ranges.iter().find(|r| {
                r.sample_format() == fmt
                    && r.min_sample_rate().0 <= 48000
                    && r.max_sample_rate().0 >= 48000
            });
            if let Some(r) = range {
                candidates.push((fmt, r.with_sample_rate(cpal::SampleRate(48000)).config()));
            }
        }
        if let Ok(def) = input_device.default_input_config() {
            let cfg = def.config();
            let dup = candidates.iter().any(|(f, c)| {
                *f == def.sample_format()
                    && c.sample_rate == cfg.sample_rate
                    && c.channels == cfg.channels
            });
            if !dup {
                candidates.push((def.sample_format(), cfg));
            }
        }
        if candidates.is_empty() {
            return Err("Не удалось получить поддерживаемый формат микрофона".into());
        }

        let name = input_device.name().unwrap_or_default();
        let mut last_err: Option<Box<dyn std::error::Error>> = None;
        for (fmt, cfg) in candidates {
            println!(
                "[АУДИО] Микрофон '{}': пробуем {:?}, {} Гц, каналов: {}",
                name, fmt, cfg.sample_rate.0, cfg.channels
            );
            let attempt = match fmt {
                cpal::SampleFormat::F32 => {
                    Self::build_input::<f32>(&input_device, &cfg, tx.clone(), fill, epochs)
                }
                cpal::SampleFormat::I16 => {
                    Self::build_input::<i16>(&input_device, &cfg, tx.clone(), fill, epochs)
                }
                cpal::SampleFormat::I32 => {
                    Self::build_input::<i32>(&input_device, &cfg, tx.clone(), fill, epochs)
                }
                other => {
                    eprintln!("[АУДИО] Формат {:?} для микрофона не поддержан", other);
                    continue;
                }
            };
            match attempt {
                Ok(stream) => {
                    let slots = match fill {
                        [true, true] => "1+2",
                        [true, false] => "1",
                        _ => "2",
                    };
                    println!(
                        "[АУДИО] Открыт вход: '{}', частота: {} Гц, каналов: {}, формат: {:?}, слоты: {}",
                        name, cfg.sample_rate.0, cfg.channels, fmt, slots
                    );
                    return Ok(stream);
                }
                Err(e) => {
                    eprintln!("[АУДИО] Микрофон не открылся в {:?}: {}", fmt, e);
                    last_err = Some(e);
                }
            }
        }
        Err(last_err.unwrap_or_else(|| "ни один формат микрофона не открылся".into()))
    }

    /// Поток ввода в формате самой карты. Сведение каналов, ресэмплинг,
    /// обработка и Opus от типа сэмпла не зависят — как и на выводе,
    /// параметризуем, а не дублируем.
    fn build_input<T>(
        input_device: &cpal::Device,
        config: &cpal::StreamConfig,
        tx: crate::txbus::TxContext,
        fill: [bool; 2],
        // Эпоха фиксируется при сборке потока: у старого колбэка после
        // перезапуска она другая, и его сэмплы в шину уже не попадут.
        epochs: [u64; 2],
    ) -> Result<cpal::Stream, Box<dyn std::error::Error>>
    where
        T: cpal::SizedSample + cpal::FromSample<f32>,
        f32: cpal::FromSample<T>,
    {
        let sample_rate = config.sample_rate.0;
        let total_channels = config.channels as usize;

        // У каждого слота свой ресэмплер: с общей позицией два независимых
        // потока из одного колбэка съедали бы по сэмплу на каждой границе.
        let mut mono: [Vec<f32>; 2] = [Vec::with_capacity(1024), Vec::with_capacity(1024)];
        let mut resample_pos: [f64; 2] = [0.0, 0.0];

        let stream = input_device.build_input_stream(
            config,
            move |data: &[T], _| {
                let mixer = &tx.mixer;
                for slot in 0..2 {
                    if !fill[slot] {
                        continue;
                    }
                    let channel = if slot == 0 {
                        mixer.input1_channel()
                    } else {
                        mixer.input2_channel()
                    };
                    let volume = if slot == 0 {
                        mixer.ch1_volume()
                    } else {
                        mixer.ch2_volume()
                    };
                    mono[slot].clear();

                    // Индекс вне диапазона — «вход выключен». На канал 0 не
                    // откатываемся: моно-микрофон с сохранённым вторым каналом
                    // иначе попал бы в оба слота и звучал вдвое громче.
                    if !channel_usable(channel, total_channels) {
                        if slot == 0 {
                            mixer.set_input1_peak_raw(0);
                        } else {
                            mixer.set_input2_peak_raw(0);
                        }
                        continue;
                    }

                    // VU — сырой пик канала до усиления, как раньше.
                    let ch = channel as usize;
                    let gain = volume as f32 / 100.0;
                    let mut peak = 0.0f32;
                    for frame in data.chunks_exact(total_channels) {
                        let v = <f32 as cpal::FromSample<T>>::from_sample_(frame[ch]);
                        peak = peak.max(v.abs());
                        mono[slot].push((v * gain).clamp(-1.0, 1.0));
                    }
                    if slot == 0 {
                        mixer.set_input1_peak_raw((peak * 100.0) as u32);
                    } else {
                        mixer.set_input2_peak_raw((peak * 100.0) as u32);
                    }

                    // Карта не на 48 кГц — приводим к 48 кГц здесь, чтобы шина
                    // смешивала сэмплы из одного времени.
                    if sample_rate != 48000 {
                        let normalized =
                            resample_linear(&mono[slot], sample_rate, 48000, &mut resample_pos[slot]);
                        tx.bus.push(slot, epochs[slot], &normalized);
                    } else {
                        tx.bus.push(slot, epochs[slot], &mono[slot]);
                    }
                }
            },
            move |err| {
                eprintln!("[ОШИБКА] Ошибка входного аудиопотока: {}", err);
            },
            None,
        )?;

        stream.play()?;
        Ok(stream)
    }

    pub fn create_playback_stream(
        device: Option<cpal::Device>,
        rx: std::sync::Arc<std::sync::Mutex<tokio::sync::mpsc::Receiver<(u64, u32, Vec<u8>)>>>,
        mixer: std::sync::Arc<crate::mixer::AudioMixer>,
        diag: std::sync::Arc<crate::diagnostics::Diagnostics>,
    ) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let output_device = match device {
            Some(d) => d,
            None => host
                .default_output_device()
                .ok_or("No default output device")?,
        };

        // Раньше брали первый диапазон, в который влезает 48 кГц, не глядя на
        // формат сэмплов, и всегда строили поток из f32. Устройство, которое
        // принимает только i16 (часть USB/HDMI и многоканальных виртуальных
        // карт), отвечало StreamConfigNotSupported: поток не открывался, и
        // собеседника не было слышно вовсе. Теперь формат берём у самой карты.
        let ranges: Vec<cpal::SupportedStreamConfigRange> = output_device
            .supported_output_configs()
            .map(|configs| configs.collect())
            .unwrap_or_default();

        // i32 нужен из-за виртуальных 7.1-устройств (Razer 7.1 Surround Sound):
        // там драйвер отдаёт f32, но воспроизводит его перекошенным — речь
        // превращается в свист, зато тот же поток в i32 звучит нормально
        // (RustAudio/cpal#892). Поэтому i32 тоже в списке кандидатов.
        let mut candidates: Vec<(cpal::SampleFormat, u32, u16)> = Vec::new();
        for format in [
            cpal::SampleFormat::F32,
            cpal::SampleFormat::I32,
            cpal::SampleFormat::I16,
        ] {
            for range in &ranges {
                if range.sample_format() != format
                    || range.min_sample_rate().0 > 48000
                    || range.max_sample_rate().0 < 48000
                {
                    continue;
                }
                let cand = (format, 48000u32, range.channels());
                if !candidates.contains(&cand) {
                    candidates.push(cand);
                }
            }
        }
        // Меньше каналов — лучше: стерео на виртуальном 7.1-устройстве звучит
        // как обычные динамики, а восьмиканальный поток та же виртуализация
        // размазывает по комнате, и речь превращается в кашу. На многоканальном
        // выходе ставим i32 впереди f32: у таких виртуальных карт f32 — как раз
        // тот формат, который они портят.
        let format_order = |format: cpal::SampleFormat, channels: u16| -> u8 {
            if channels > 2 {
                match format {
                    cpal::SampleFormat::I32 => 0,
                    cpal::SampleFormat::F32 => 1,
                    cpal::SampleFormat::I16 => 2,
                    _ => 3,
                }
            } else {
                match format {
                    cpal::SampleFormat::F32 => 0,
                    cpal::SampleFormat::I16 => 1,
                    cpal::SampleFormat::I32 => 2,
                    _ => 3,
                }
            }
        };
        candidates.sort_by_key(|(format, _, channels)| {
            (*channels, format_order(*format, *channels))
        });

        // Последний шанс — формат устройства по умолчанию, на какой бы частоте
        // он ни был: ресэмплер ниже приведёт звук к нему.
        if let Ok(default) = output_device.default_output_config() {
            let cand = (
                default.sample_format(),
                default.sample_rate().0,
                default.channels(),
            );
            if !candidates.contains(&cand) {
                candidates.push(cand);
            }
        }

        for (format, rate, channels) in candidates {
            let config = cpal::StreamConfig {
                channels,
                sample_rate: cpal::SampleRate(rate),
                buffer_size: cpal::BufferSize::Default,
            };
            let built = match format {
                cpal::SampleFormat::F32 => Self::build_playback::<f32>(
                    &output_device,
                    &config,
                    rx.clone(),
                    mixer.clone(),
                    diag.clone(),
                ),
                cpal::SampleFormat::I32 => Self::build_playback::<i32>(
                    &output_device,
                    &config,
                    rx.clone(),
                    mixer.clone(),
                    diag.clone(),
                ),
                cpal::SampleFormat::I16 => Self::build_playback::<i16>(
                    &output_device,
                    &config,
                    rx.clone(),
                    mixer.clone(),
                    diag.clone(),
                ),
                _ => continue,
            };
            match built {
                Ok(stream) => {
                    println!(
                        "[АУДИО] Открыты динамики: '{}', частота: {} Гц, каналов: {}, формат: {:?}",
                        output_device.name().unwrap_or_default(),
                        rate,
                        channels,
                        format
                    );
                    return Ok(stream);
                }
                Err(e) => eprintln!(
                    "[АУДИО] Динамики: формат {:?} / {} Гц / {} каналов не подошёл: {e}",
                    format, rate, channels
                ),
            }
        }
        Err("ни один формат динамиков не открылся".into())
    }

    /// Поток вывода в формате самой карты (f32 там, где он есть, иначе i16).
    /// Приём, джиттер-буфер, декодеры и маскирование от типа сэмпла не зависят,
    /// поэтому параметризованы здесь, а не продублированы.
    fn build_playback<T>(
        output_device: &cpal::Device,
        config: &cpal::StreamConfig,
        rx: std::sync::Arc<std::sync::Mutex<tokio::sync::mpsc::Receiver<(u64, u32, Vec<u8>)>>>,
        mixer: std::sync::Arc<crate::mixer::AudioMixer>,
        diag: std::sync::Arc<crate::diagnostics::Diagnostics>,
    ) -> Result<cpal::Stream, Box<dyn std::error::Error>>
    where
        T: cpal::SizedSample + cpal::FromSample<f32>,
    {
        let sample_rate = config.sample_rate.0;
        let output_channels = config.channels as usize;
        // На многоканальном устройстве пишем только в первые два канала:
        // одинаковый сигнал во всех восьми виртуальный 7.1 размазывает и
        // добавляет кашу. Остальные каналы держим в нуле.
        let active_channels = output_channels.min(2);

        // Приём ведётся отдельно на каждого собеседника: у Opus своё
        // внутреннее состояние (SILK/CELT), и один общий декодер на всех
        // смешивал потоки — как только в комнате звучал второй голос, речь
        // превращалась в «робота». Своими у каждого и позиция ресэмплера, и
        // порядок seq, и последний кадр (им маскируются потери).
        struct Peer {
            decoder: rusty_opus::OpusDecoder,
            resample_pos: f64,
            next: u32,
            queue: std::collections::BTreeMap<u32, Vec<u8>>,
            stale: u32,
            last_pcm: Vec<f32>,
            /// Декодированный (уже с персональной громкостью) поток этого
            /// собеседника, ещё не сведённый в общий микс.
            pcm: Vec<f32>,
            /// Набрался ли предбуфер: до этого голос молчит, чтобы поток
            /// не дёргался на первых пакетах.
            started: bool,
            /// Когда пришёл последний кадр: по нему отличаем провал буфера
            /// посреди реплики от нормального конца (PTT отпущен).
            last_fill: std::time::Instant,
            /// Предбуфер этого собеседника в сэмплах. Растёт, если его звук
            /// рвётся (разовые провалы на плохом Wi-Fi), и плавно возвращается
            /// к базовым 60 мс, когда канал успокоился: задержка не должна
            /// оставаться высокой из-за старого сбоя.
            prebuffer: usize,
            /// Когда был последний провал — по нему решаем, пора ли снижать
            /// предбуфер обратно.
            last_underrun: Option<std::time::Instant>,
        }

        let mut mono_decoded = [0.0f32; 960];
        // кадр для карты: сначала собираем в f32, в конце переводим в её тип
        let mut out_scratch: Vec<f32> = Vec::new();
        // один лимитер на весь микс: сколько бы голосов ни сложилось, на выходе
        // не больше −1 dBFS. Персональный +18 дБ при этом доходит до слуха —
        // раньше его съедал лимитер каждого отдельного голоса.
        let mut mix_limiter = crate::mixer::Limiter::new(sample_rate);
        // монитор своего тракта приходит на 48 кГц, карта может быть другой
        let mut monitor_in: Vec<f32> = Vec::new();
        let mut monitor_pos: f64 = 0.0;
        // позиция ресэмплера опоры эха (карта не на 48 кГц)
        let mut echo_pos: f64 = 0.0;
        // гудок звонка синтезируется прямо в колбэке: без файлов и без задержки
        let mut ring_t: f64 = 0.0;
        let mut last_ring_mode: u32 = 0;
        // короткий тон с плавными краями: без фейдов слышны щелчки
        let ring_tone = |t: f64, freq: f64, from: f64, to: f64| -> f32 {
            if t < from || t >= to {
                return 0.0;
            }
            let fade = 0.010;
            let env = ((t - from) / fade)
                .min(1.0)
                .min(((to - t) / fade).min(1.0))
                .max(0.0);
            ((2.0 * std::f64::consts::PI * freq * t).sin() * env * 0.10) as f32
        };
        let mut peers: std::collections::HashMap<u64, Peer> = std::collections::HashMap::new();
        // Диагностика приёма: те же события видны и в консоли (раз в 10 с), и в
        // панели «Диагностика» — там счётчики накопительные, поэтому журнал
        // печатает разницу с прошлым отчётом.
        let mut last_report = std::time::Instant::now();
        let mut last_printed = diag.rx.snapshot();

        let stream = output_device.build_output_stream(
            config,
            move |data: &mut [T], _| {
                // Принимаем входящие пакеты, переупорядочиваем по seq и декодируем
                if let Ok(mut rx_lock) = rx.try_lock() {
                    while let Ok((sender_id, seq, packet)) = rx_lock.try_recv() {
                        if !peers.contains_key(&sender_id) {
                            let Ok(decoder) = rusty_opus::OpusDecoder::new(48000, 1) else {
                                continue;
                            };
                            peers.insert(
                                sender_id,
                                Peer {
                                    decoder,
                                    resample_pos: 0.0,
                                    next: seq,
                                    queue: std::collections::BTreeMap::new(),
                                    stale: 0,
                                    last_pcm: Vec::new(),
                                    pcm: Vec::new(),
                                    started: false,
                                    last_fill: std::time::Instant::now(),
                                    prebuffer: (sample_rate as usize * 6) / 100,
                                    last_underrun: None,
                                },
                            );
                        }
                        let peer = peers.get_mut(&sender_id).unwrap();

                        // Отбрасываем устаревшие/дублированные кадры. Отправитель,
                        // перезапустивший приложение, начинает seq с нуля: замечаем
                        // это по пачке «устаревших» подряд и заводим его заново —
                        // иначе, пока говорят другие, его не слышно вообще.
                        if seq.wrapping_sub(peer.next) >= 0x8000_0000 {
                            peer.stale += 1;
                            if peer.stale < 25 {
                                continue;
                            }
                            peer.next = seq;
                            peer.queue.clear();
                            peer.stale = 0;
                            diag.rx.restart();
                        }
                        peer.stale = 0;
                        peer.queue.insert(seq, packet);

                        // Дырка в нумерации: один потерянный датаграм не должен
                        // останавливать воспроизведение. Иначе звук замирает, пока
                        // не наберётся полный буфер, и выплёвывается пачкой — на слух
                        // это «робот». Но и повтор последнего кадра «роботом» звучит:
                        // одна и та же волна дважды. Поэтому восстанавливаем кадр
                        // по порядку цены и качества:
                        //   1) in-band FEC — Opus кладёт в следующий пакет копию
                        //      предыдущего кадра (SILK/hybrid), берём её;
                        //   2) PLC декодера — продолжает сигнал по модели (тон и
                        //      форма спектра), а не повторяет кадр;
                        //   3) повтор с затуханием — только если декодер ещё ничего
                        //      не слышал и восстанавливать нечем.
                        if !peer.queue.contains_key(&peer.next) && peer.queue.len() >= 3 {
                            if let Some(&lowest) = peer.queue.keys().next() {
                                let missing = lowest.wrapping_sub(peer.next) as usize;
                                if missing <= 3 && !peer.last_pcm.is_empty() {
                                    for r in 0..missing {
                                        let mut restored = [0.0f32; 960];
                                        // FEC отдаёт ровно один кадр — тот, что был
                                        // до следующего пакета; остальные дырки
                                        // закрывает PLC.
                                        let len = if r == 0 {
                                            match peer.queue.get(&lowest) {
                                                Some(next) => match peer.decoder.decode_fec(
                                                    next,
                                                    960,
                                                    &mut restored,
                                                ) {
                                                    Ok(n) if n > 0 => {
                                                        diag.rx.fec();
                                                        n
                                                    }
                                                    _ => plc(&mut peer.decoder, &mut restored),
                                                },
                                                None => plc(&mut peer.decoder, &mut restored),
                                            }
                                        } else {
                                            plc(&mut peer.decoder, &mut restored)
                                        };
                                        if len == 0 {
                                            let fade =
                                                1.0 - (r as f32 + 1.0) / (missing as f32 + 1.0);
                                            peer.pcm
                                                .extend(peer.last_pcm.iter().map(|s| s * fade));
                                        } else {
                                            mixer.process_output(sender_id, &mut restored[..len]);
                                            let chunk: Vec<f32> = if sample_rate != 48000 {
                                                resample_linear(
                                                    &restored[..len],
                                                    48000,
                                                    sample_rate,
                                                    &mut peer.resample_pos,
                                                )
                                            } else {
                                                restored[..len].to_vec()
                                            };
                                            peer.pcm.extend_from_slice(&chunk);
                                        }
                                        diag.rx.concealed();
                                    }
                                }
                                peer.next = lowest;
                            }
                        }

                        // Не даём буферу переупорядочивания расти при большой дыре в потоке
                        while peer.queue.len() > 32 {
                            let oldest = *peer.queue.keys().next().unwrap();
                            peer.queue.remove(&oldest);
                            peer.next = oldest.wrapping_add(1);
                            diag.rx.drop();
                        }

                        // Выдаём все кадры строго по порядку
                        while let Some(packet) = peer.queue.remove(&peer.next) {
                            peer.next = peer.next.wrapping_add(1);
                            let Ok(len) = peer.decoder.decode(&packet, 960, &mut mono_decoded)
                            else {
                                continue;
                            };
                            mixer.process_output(sender_id, &mut mono_decoded[..len]);

                            // Если звуковая карта работает не на 48 кГц (например 44.1 или 96 кГц),
                            // ресэмплируем декодированный звук строго под частоту звуковой карты!
                            let chunk: Vec<f32> = if sample_rate != 48000 {
                                resample_linear(
                                    &mono_decoded[..len],
                                    48000,
                                    sample_rate,
                                    &mut peer.resample_pos,
                                )
                            } else {
                                mono_decoded[..len].to_vec()
                            };
                            peer.pcm.extend_from_slice(&chunk);
                            peer.last_fill = std::time::Instant::now();
                            peer.last_pcm = chunk;
                        }
                    }
                }

                // Раз в 10 с пишем в консоль, если на приёме что-то терялось:
                // по этим числам видно, из-за чего у собеседника «робот» —
                // дырки в сети, перезапуск отправителя или провалы звуковой карты.
                if last_report.elapsed() >= std::time::Duration::from_secs(10) {
                    let now = diag.rx.snapshot();
                    let window = last_printed.delta(&now);
                    if window.events() > 0 {
                        eprintln!(
                            "[АУДИО] приём: провалов буфера {}, маскировано кадров {}, перезапусков потока {}, выброшено пачкой {}",
                            window.underruns, window.concealed, window.restarts, window.drops
                        );
                    }
                    last_printed = now;
                    last_report = std::time::Instant::now();
                }

                // ————— Сведение —————
                // Раньше куски всех собеседников дописывались в один буфер
                // подряд: когда говорили двое, кадры шли друг за другом во
                // времени, а не поверх — очередь росла и её хвост выбрасывался,
                // то есть речь одного человека глушила другого. Теперь у каждого
                // свой буфер, кадры складываются поверх, а лишнее режется по
                // каждому собеседнику отдельно.
                let num_frames = data.len() / output_channels;
                out_scratch.clear();
                out_scratch.resize(num_frames, 0.0);

                let base_prebuffer = (sample_rate as usize * 6) / 100; // 60 мс
                let max_prebuffer = (sample_rate as usize * 15) / 100; // 150 мс
                let max_buffer = (sample_rate as usize * 20) / 100; // 200 мс
                let keep_buffer = (sample_rate as usize * 8) / 100; // 80 мс

                for peer in peers.values_mut() {
                    if peer.pcm.len() > max_buffer {
                        let excess = peer.pcm.len() - keep_buffer;
                        peer.pcm.drain(..excess);
                        diag.rx.drop();
                    }
                    // Канал успокоился — возвращаемся к базовой задержке.
                    // Понижаем ступенью, а не сразу: резкое уменьшение буфера
                    // само по себе даёт провал.
                    if peer.prebuffer > base_prebuffer {
                        let calm = peer
                            .last_underrun
                            .map(|at| at.elapsed() >= std::time::Duration::from_secs(20))
                            .unwrap_or(true);
                        if calm {
                            peer.prebuffer = peer
                                .prebuffer
                                .saturating_sub(sample_rate as usize / 100)
                                .max(base_prebuffer);
                        }
                    }
                    if !peer.started {
                        if peer.pcm.len() >= peer.prebuffer {
                            peer.started = true;
                        } else {
                            continue;
                        }
                    }
                    if peer.pcm.is_empty() {
                        peer.started = false;
                        // Провал посреди реплики — это потеря; отпущенный PTT
                        // так же оставляет буфер пустым, но это норма.
                        if peer.last_fill.elapsed() < std::time::Duration::from_millis(300) {
                            diag.rx.underrun();
                            peer.last_underrun = Some(std::time::Instant::now());
                            if peer.prebuffer < max_prebuffer {
                                peer.prebuffer = (peer.prebuffer + sample_rate as usize / 50)
                                    .min(max_prebuffer);
                                eprintln!(
                                    "[АУДИО] Провал на приёме — предбуфер поднят до {} мс",
                                    peer.prebuffer * 1000 / sample_rate as usize
                                );
                            }
                        }
                        continue;
                    }
                    let n = peer.pcm.len().min(num_frames);
                    for (dst, src) in out_scratch.iter_mut().zip(peer.pcm.drain(..n)) {
                        *dst += src;
                    }
                }

                // Общая громкость и один лимитер на готовый микс
                let master_gain = mixer.master_gain();
                for sample in out_scratch.iter_mut() {
                    *sample *= master_gain;
                }

                // Опора эхо-подавления — ровно то, что услышат колонки из
                // голосов собеседников. Монитор и гудок сюда не входят: свой
                // голос из микрофона вычитать нельзя. AEC3 работает на 48 кГц,
                // поэтому карту на другой частоте приводим здесь же.
                if sample_rate != 48000 {
                    let ref48 =
                        resample_linear(&out_scratch, sample_rate, 48000, &mut echo_pos);
                    mixer.push_echo_ref(&ref48);
                } else {
                    mixer.push_echo_ref(&out_scratch);
                }

                // Свой тракт подмешивается после общей громкости: его уровень
                // не должен зависеть от того, насколько прибавлены собеседники
                if mixer.is_monitor_enabled() {
                    let need = ((num_frames as f64) * 48_000.0 / sample_rate as f64).ceil() as usize
                        + 2;
                    mixer.drain_monitor(&mut monitor_in, need);
                    if !monitor_in.is_empty() {
                        let chunk: Vec<f32> = if sample_rate != 48000 {
                            resample_linear(&monitor_in, 48000, sample_rate, &mut monitor_pos)
                        } else {
                            monitor_in.clone()
                        };
                        for (dst, src) in out_scratch.iter_mut().zip(chunk) {
                            *dst += src;
                        }
                    }
                }

                // Гудок звонка: входящий — двойной сигнал раз в 2.2 с,
                // исходящий — длинный гудок раз в 4 с. Слышен независимо от
                // громкости собеседников: его нельзя случайно «замьютить».
                let ring_mode = mixer.ring_mode();
                if ring_mode != last_ring_mode {
                    ring_t = 0.0;
                    last_ring_mode = ring_mode;
                }
                if ring_mode != 0 {
                    let dt = 1.0 / sample_rate as f64;
                    let period = if ring_mode == 1 { 2.2 } else { 4.0 };
                    for (i, sample) in out_scratch.iter_mut().enumerate() {
                        let t = (ring_t + i as f64 * dt) % period;
                        let tone = if ring_mode == 1 {
                            ring_tone(t, 880.0, 0.0, 0.20) + ring_tone(t, 880.0, 0.32, 0.52)
                        } else {
                            ring_tone(t, 425.0, 0.0, 0.8)
                        };
                        *sample += tone;
                    }
                    ring_t = (ring_t + num_frames as f64 * dt) % period;
                }

                // Лимитер микса — опция (по умолчанию выключен): без него
                // громкость приёма ровно такая, как выставили ползунками.
                if mixer.is_rx_limiter_enabled() {
                    mix_limiter.process(&mut out_scratch);
                }

                let silence = T::from_sample(0.0);
                for (i, frame) in out_scratch.iter().enumerate() {
                    let value = T::from_sample(*frame);
                    let base = i * output_channels;
                    for ch in 0..active_channels {
                        data[base + ch] = value;
                    }
                    for ch in active_channels..output_channels {
                        data[base + ch] = silence;
                    }
                }
            },
            |e| eprintln!("[ОШИБКА] Ошибка выходного аудиопотока: {}", e),
            None,
        )?;

        stream.play()?;
        Ok(stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_bounds() {
        // Выключенный канал
        assert!(!channel_usable(u32::MAX, 2));
        // Существующие каналы
        assert!(channel_usable(0, 2));
        assert!(channel_usable(1, 2));
        // Второй канал у моно-микрофона: молчим, а не дублируем первый
        assert!(!channel_usable(1, 1));
        // Пустое устройство каналов не имеет
        assert!(!channel_usable(0, 0));
    }
}


