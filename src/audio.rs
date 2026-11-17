use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct UniversalAudioCapture;

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

impl UniversalAudioCapture {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(UniversalAudioCapture)
    }

    pub fn create_input_stream(
        &mut self,
        device: Option<cpal::Device>,
        ptt_flag: Arc<AtomicBool>,
        packet_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
        mixer: std::sync::Arc<crate::mixer::AudioMixer>,
    ) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let input_device = match device {
            Some(d) => d,
            None => host
                .default_input_device()
                .ok_or("No default input device")?,
        };

        // Ищем конфигурацию на 48000 Гц, предпочитая f32: микрофонный поток
        // строится с типом f32, и у карты, которая на 48 кГц даёт только i16,
        // первый подходящий по частоте диапазон приводил к отказу.
        let ranges: Vec<cpal::SupportedStreamConfigRange> = input_device
            .supported_input_configs()
            .map(|configs| configs.collect())
            .unwrap_or_default();
        let at_48000 = |f32_only: bool, range: &&cpal::SupportedStreamConfigRange| {
            range.min_sample_rate().0 <= 48000
                && range.max_sample_rate().0 >= 48000
                && (!f32_only || range.sample_format() == cpal::SampleFormat::F32)
        };
        let best_config = ranges
            .iter()
            .find(|r| at_48000(true, r))
            .or_else(|| ranges.iter().find(|r| at_48000(false, r)))
            .map(|r| {
                r.with_sample_rate(cpal::SampleRate(48000))
                    .config()
            })
            .or_else(|| input_device.default_input_config().ok().map(|c| c.config()))
            .ok_or("Не удалось получить поддерживаемый формат микрофона")?;

        let sample_rate = best_config.sample_rate.0;
        let total_channels = best_config.channels as usize;

        println!(
            "[АУДИО] Открыт микрофон: '{}', частота: {} Гц, каналов: {}",
            input_device.name().unwrap_or_default(),
            sample_rate,
            total_channels
        );

        // Создаем Opus-энкодер строго на 48000 Гц, 64 kbps
        let mut encoder = rusty_opus::OpusEncoder::new(48000, 1, rusty_opus::Application::Voip)?;
        encoder.bitrate_bps = 64_000;

        let mut sample_acc: Vec<f32> = Vec::with_capacity(1920);
        let mut mono_chunk: Vec<f32> = Vec::with_capacity(960);
        let mut resample_pos: f64 = 0.0;
        let mut gate_gain: f32 = 0.0;

        let stream = input_device.build_input_stream(
            &best_config,
            move |data: &[f32], _| {
                mono_chunk.clear();
                let ch1 = mixer.input1_channel();
                let ch2 = mixer.input2_channel();
                let gain1 = mixer.ch1_volume() as f32 / 100.0;
                let gain2 = mixer.ch2_volume() as f32 / 100.0;

                let mut peak1 = 0.0f32;
                let mut peak2 = 0.0f32;

                // Сводим выбранные входы в моно с независимым усилением.
                // Нормируем по числу активных каналов, чтобы сумма не превышала 1.0
                // (иначе при двух каналах на 100% уровень удваивается и жёстко клиппится).
                for chunk in data.chunks_exact(total_channels) {
                    let mut sample = 0.0f32;
                    let mut active = 0u32;
                    if (ch1 as usize) < total_channels {
                        let v1 = chunk[ch1 as usize];
                        peak1 = peak1.max(v1.abs());
                        if gain1 > 0.0 {
                            sample += v1 * gain1;
                            active += 1;
                        }
                    }
                    if (ch2 as usize) < total_channels {
                        let v2 = chunk[ch2 as usize];
                        peak2 = peak2.max(v2.abs());
                        if gain2 > 0.0 {
                            sample += v2 * gain2;
                            active += 1;
                        }
                    }
                    if active > 1 {
                        sample /= active as f32;
                    }
                    mono_chunk.push(sample.clamp(-1.0, 1.0));
                }

                mixer.set_input1_peak_raw((peak1 * 100.0) as u32);
                mixer.set_input2_peak_raw((peak2 * 100.0) as u32);

                // Общий мьют
                mixer.process_input(&mut mono_chunk);

                // Если микрофон работает не на 48 кГц, ресэмплируем строго в 48 000 Гц
                let normalized_chunk = if sample_rate != 48000 {
                    resample_linear(&mono_chunk, sample_rate, 48000, &mut resample_pos)
                } else {
                    mono_chunk.clone()
                };

                // Noise Gate (Шумовой затвор): порог ~ -44 dBFS (0.006)
                let mut processed_chunk = normalized_chunk;
                let chunk_peak = processed_chunk
                    .iter()
                    .map(|s| s.abs())
                    .fold(0.0f32, f32::max);
                let target_gain = if chunk_peak > 0.006 { 1.0f32 } else { 0.0f32 };

                for sample in processed_chunk.iter_mut() {
                    if target_gain > gate_gain {
                        gate_gain += (target_gain - gate_gain) * 0.1;
                    } else {
                        gate_gain += (target_gain - gate_gain) * 0.002;
                    }
                    *sample *= gate_gain;
                }

                // Нарезаем ровно по 960 сэмплов (20 мс при 48 кГц) для Opus
                if ptt_flag.load(Ordering::Relaxed) {
                    sample_acc.extend_from_slice(&processed_chunk);
                    while sample_acc.len() >= 960 {
                        let frame: Vec<f32> = sample_acc.drain(..960).collect();
                        let mut packet_buf = vec![0u8; 1024];
                        if let Ok(encoded_len) = encoder.encode(&frame, 960, &mut packet_buf) {
                            let _ = packet_tx.try_send(packet_buf[..encoded_len].to_vec());
                        }
                    }
                    if sample_acc.len() > 4800 {
                        sample_acc.clear();
                    }
                } else {
                    sample_acc.clear();
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

        let mut candidates: Vec<(cpal::SampleFormat, u32, u16)> = Vec::new();
        for format in [cpal::SampleFormat::F32, cpal::SampleFormat::I16] {
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
                cpal::SampleFormat::F32 => {
                    Self::build_playback::<f32>(&output_device, &config, rx.clone(), mixer.clone())
                }
                cpal::SampleFormat::I16 => {
                    Self::build_playback::<i16>(&output_device, &config, rx.clone(), mixer.clone())
                }
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
    ) -> Result<cpal::Stream, Box<dyn std::error::Error>>
    where
        T: cpal::SizedSample + cpal::FromSample<f32>,
    {
        let sample_rate = config.sample_rate.0;
        let output_channels = config.channels as usize;

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
        }

        let mut mono_decoded = [0.0f32; 960];
        let mut pcm_buffer: Vec<f32> = Vec::with_capacity(3840);
        // кадр для карты: сначала собираем в f32, в конце переводим в её тип
        let mut out_scratch: Vec<f32> = Vec::new();
        let mut prebuffered = false;
        let mut peers: std::collections::HashMap<u64, Peer> = std::collections::HashMap::new();
        // диагностика приёма: по этим числам видно, что именно рвёт звук
        let (mut underruns, mut concealed, mut stale_resets, mut burst_drops) =
            (0u32, 0u32, 0u32, 0u32);
        let mut last_report = std::time::Instant::now();

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
                            stale_resets += 1;
                        }
                        peer.stale = 0;
                        peer.queue.insert(seq, packet);

                        // Дырка в нумерации: один потерянный датаграм не должен
                        // останавливать воспроизведение. Иначе звук замирает, пока
                        // не наберётся полный буфер, и выплёвывается пачкой — на слух
                        // это «робот». Если впереди уже несколько пакетов, ждать
                        // потерянный бессмысленно: считаем его потерянным и
                        // маскируем: повторяем последний кадр с затуханием.
                        if !peer.queue.contains_key(&peer.next) && peer.queue.len() >= 3 {
                            if let Some(&lowest) = peer.queue.keys().next() {
                                let missing = lowest.wrapping_sub(peer.next) as usize;
                                if missing <= 3 && !peer.last_pcm.is_empty() {
                                    for r in 0..missing {
                                        let fade = 1.0 - (r as f32 + 1.0) / (missing as f32 + 1.0);
                                        pcm_buffer
                                            .extend(peer.last_pcm.iter().map(|s| s * fade));
                                        concealed += 1;
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
                            burst_drops += 1;
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
                            pcm_buffer.extend_from_slice(&chunk);
                            peer.last_pcm = chunk;
                        }
                    }
                }

                // Сброс хвоста джиттер-буфера при лагах сети: держим до 200 мс,
                // а лишнее убираем до 80 мс — большую пачку выкидывать больнее,
                // чем небольшую (это слышно как провал).
                let max_buffer = (sample_rate as usize * 20) / 100; // 200 мс
                if pcm_buffer.len() > max_buffer {
                    let keep = (sample_rate as usize * 8) / 100; // 80 мс
                    let excess = pcm_buffer.len().saturating_sub(keep);
                    pcm_buffer.drain(..excess);
                }

                // Раз в 10 с пишем в консоль, если на приёме что-то терялось:
                // по этим числам видно, из-за чего у собеседника «робот» —
                // дырки в сети, перезапуск отправителя или провалы звуковой карты.
                if last_report.elapsed() >= std::time::Duration::from_secs(10) {
                    if underruns + concealed + stale_resets + burst_drops > 0 {
                        eprintln!(
                            "[АУДИО] приём: провалов буфера {underruns}, маскировано кадров {concealed}, перезапусков потока {stale_resets}, выброшено пачкой {burst_drops}"
                        );
                        underruns = 0;
                        concealed = 0;
                        stale_resets = 0;
                        burst_drops = 0;
                    }
                    last_report = std::time::Instant::now();
                }

                // Кадр собираем в f32 (нули — тишина) и в конце переводим в тип
                // карты: у i16-устройства данные приходят другим типом.
                let num_frames = data.len() / output_channels;
                out_scratch.clear();
                out_scratch.resize(num_frames, 0.0);
                let min_prebuffer = (sample_rate as usize * 6) / 100; // 60 мс

                if !prebuffered && pcm_buffer.len() >= min_prebuffer {
                    prebuffered = true;
                }
                if prebuffered && pcm_buffer.is_empty() {
                    prebuffered = false;
                    underruns += 1;
                }
                if prebuffered {
                    let available_frames = pcm_buffer.len().min(num_frames);
                    out_scratch[..available_frames]
                        .copy_from_slice(&pcm_buffer[..available_frames]);
                    pcm_buffer.drain(..available_frames);
                }

                for (i, frame) in out_scratch.iter().enumerate() {
                    let value = T::from_sample(*frame);
                    let base = i * output_channels;
                    for ch in 0..output_channels {
                        data[base + ch] = value;
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
