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

    while *position < input.len() as f64 - 1.0 {
        let idx = *position as usize;
        let frac = (*position - idx as f64) as f32;
        let s0 = input[idx];
        let s1 = input[idx + 1];
        output.push(s0 + frac * (s1 - s0));
        *position += ratio;
    }

    *position -= (input.len().saturating_sub(1)) as f64;
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

        // Ищем конфигурацию, поддерживающую 48000 Гц, либо берем дефолтную
        let supported_configs = input_device.supported_input_configs().ok();
        let best_config = supported_configs
            .and_then(|mut configs| {
                configs
                    .find(|c| c.min_sample_rate().0 <= 48000 && c.max_sample_rate().0 >= 48000)
                    .map(|c| c.with_sample_rate(cpal::SampleRate(48000)).config())
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

        // Запрашиваем 48000 Гц у динамиков, либо берем их дефолтный формат
        let supported_configs = output_device.supported_output_configs().ok();
        let best_config = supported_configs
            .and_then(|mut configs| {
                configs
                    .find(|c| c.min_sample_rate().0 <= 48000 && c.max_sample_rate().0 >= 48000)
                    .map(|c| c.with_sample_rate(cpal::SampleRate(48000)).config())
            })
            .or_else(|| {
                output_device
                    .default_output_config()
                    .ok()
                    .map(|c| c.config())
            })
            .ok_or("Не удалось получить поддерживаемый формат динамиков")?;

        let sample_rate = best_config.sample_rate.0;
        let output_channels = best_config.channels as usize;

        println!(
            "[АУДИО] Открыты динамики: '{}', частота: {} Гц, каналов: {}",
            output_device.name().unwrap_or_default(),
            sample_rate,
            output_channels
        );

        let mut decoder = rusty_opus::OpusDecoder::new(48000, 1)?;
        let mut mono_decoded = [0.0f32; 960];
        let mut pcm_buffer: Vec<f32> = Vec::with_capacity(3840);
        let mut resample_pos: f64 = 0.0;
        let mut prebuffered = false;
        let mut reorder: std::collections::HashMap<
            u64,
            (u32, std::collections::BTreeMap<u32, Vec<u8>>),
        > = std::collections::HashMap::new();

        let stream = output_device.build_output_stream(
            &best_config,
            move |data: &mut [f32], _| {
                // Принимаем входящие пакеты, переупорядочиваем по seq и декодируем
                if let Ok(mut rx_lock) = rx.try_lock() {
                    while let Ok((sender_id, seq, packet)) = rx_lock.try_recv() {
                        let entry = reorder
                            .entry(sender_id)
                            .or_insert_with(|| (seq, std::collections::BTreeMap::new()));

                        // Отбрасываем устаревшие/дублированные кадры
                        if seq.wrapping_sub(entry.0) >= 0x8000_0000 {
                            continue;
                        }
                        entry.1.insert(seq, packet);

                        // Дырка в нумерации: один потерянный датаграм не должен
                        // останавливать воспроизведение. Иначе звук замирает, пока
                        // не наберётся полный буфер, и выплёвывается пачкой — на слух
                        // это «робот». Если впереди уже несколько пакетов, ждать
                        // потерянный бессмысленно: считаем его потерянным.
                        if !entry.1.contains_key(&entry.0) && entry.1.len() >= 3 {
                            if let Some(&lowest) = entry.1.keys().next() {
                                entry.0 = lowest;
                            }
                        }

                        // Не даём буферу переупорядочивания расти при большой дыре в потоке
                        while entry.1.len() > 32 {
                            let oldest = *entry.1.keys().next().unwrap();
                            entry.1.remove(&oldest);
                            entry.0 = oldest.wrapping_add(1);
                        }

                        // Выдаём все кадры строго по порядку
                        while let Some(packet) = entry.1.remove(&entry.0) {
                            entry.0 = entry.0.wrapping_add(1);
                            if let Ok(len) = decoder.decode(&packet, 960, &mut mono_decoded) {
                                mixer.process_output(sender_id, &mut mono_decoded[..len]);

                                // Если звуковая карта работает не на 48 кГц (например 44.1 или 96 кГц),
                                // ресэмплируем декодированный звук строго под частоту звуковой карты!
                                if sample_rate != 48000 {
                                    let resampled = resample_linear(
                                        &mono_decoded[..len],
                                        48000,
                                        sample_rate,
                                        &mut resample_pos,
                                    );
                                    pcm_buffer.extend_from_slice(&resampled);
                                } else {
                                    pcm_buffer.extend_from_slice(&mono_decoded[..len]);
                                }
                            }
                        }
                    }
                }

                // Сброс хвоста джиттер-буфера при лагах сети
                let max_buffer = (sample_rate as usize * 12) / 100; // 120 мс
                if pcm_buffer.len() > max_buffer {
                    let keep = (sample_rate as usize * 4) / 100; // 40 мс
                    let excess = pcm_buffer.len().saturating_sub(keep);
                    pcm_buffer.drain(..excess);
                }

                let num_frames = data.len() / output_channels;
                let min_prebuffer = (sample_rate as usize * 4) / 100; // 40 мс

                if !prebuffered {
                    if pcm_buffer.len() >= min_prebuffer {
                        prebuffered = true;
                    } else {
                        data.fill(0.0);
                        return;
                    }
                }

                if pcm_buffer.is_empty() {
                    prebuffered = false;
                    // После перезапуска отправителя его seq сбрасывается в 0, а старый
                    // reorder-буфер считает такие кадры «устаревшими» и дропает их —
                    // собеседника перестаёт быть слышно. Сброс по недобору буфера
                    // лечит это и ускоряет восстановление после потерь.
                    reorder.clear();
                    data.fill(0.0);
                    return;
                }

                let available_frames = pcm_buffer.len().min(num_frames);

                for i in 0..available_frames {
                    let sample = pcm_buffer[i];
                    for ch in 0..output_channels {
                        data[i * output_channels + ch] = sample;
                    }
                }

                for i in available_frames..num_frames {
                    for ch in 0..output_channels {
                        data[i * output_channels + ch] = 0.0;
                    }
                }

                pcm_buffer.drain(..available_frames);
            },
            |e| eprintln!("[ОШИБКА] Ошибка выходного аудиопотока: {}", e),
            None,
        )?;

        stream.play()?;
        Ok(stream)
    }
}
