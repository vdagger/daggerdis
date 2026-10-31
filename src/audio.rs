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
            None => host.default_input_device().ok_or("No default input device")?,
        };

        // Ищем конфигурацию, поддерживающую 48000 Гц, либо берем дефолтную
        let supported_configs = input_device.supported_input_configs().ok();
        let best_config = supported_configs.and_then(|mut configs| {
            configs.find(|c| c.min_sample_rate().0 <= 48000 && c.max_sample_rate().0 >= 48000)
                .map(|c| c.with_sample_rate(cpal::SampleRate(48000)).config())
        }).or_else(|| input_device.default_input_config().ok().map(|c| c.config()))
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
                // Усредняем входные каналы в чистый монопоток
                for chunk in data.chunks_exact(total_channels) {
                    let sample = if total_channels > 1 {
                        (chunk[0] + chunk[1]) * 0.5
                    } else {
                        chunk[0]
                    };
                    mono_chunk.push(sample);
                }

                // Обработка громкости микрофона
                mixer.process_input(&mut mono_chunk);

                // Если микрофон работает не на 48 кГц, ресэмплируем строго в 48 000 Гц
                let normalized_chunk = if sample_rate != 48000 {
                    resample_linear(&mono_chunk, sample_rate, 48000, &mut resample_pos)
                } else {
                    mono_chunk.clone()
                };

                // Noise Gate (Шумовой затвор): порог ~ -44 dBFS (0.006)
                let mut processed_chunk = normalized_chunk;
                let chunk_peak = processed_chunk.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
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
        rx: std::sync::Arc<std::sync::Mutex<tokio::sync::mpsc::Receiver<(u64, Vec<u8>)>>>,
        mixer: std::sync::Arc<crate::mixer::AudioMixer>,
    ) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let output_device = match device {
            Some(d) => d,
            None => host.default_output_device().ok_or("No default output device")?,
        };

        // Запрашиваем 48000 Гц у динамиков, либо берем их дефолтный формат
        let supported_configs = output_device.supported_output_configs().ok();
        let best_config = supported_configs.and_then(|mut configs| {
            configs.find(|c| c.min_sample_rate().0 <= 48000 && c.max_sample_rate().0 >= 48000)
                .map(|c| c.with_sample_rate(cpal::SampleRate(48000)).config())
        }).or_else(|| output_device.default_output_config().ok().map(|c| c.config()))
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

        let stream = output_device.build_output_stream(
            &best_config,
            move |data: &mut [f32], _| {
                // Принимаем входящие пакеты и декодируем (всегда 48 кГц от Opus)
                if let Ok(mut rx_lock) = rx.try_lock() {
                    while let Ok((sender_id, packet)) = rx_lock.try_recv() {
                        if let Ok(len) = decoder.decode(&packet, 960, &mut mono_decoded) {
                            mixer.process_output(sender_id, &mut mono_decoded[..len]);
                            
                            // Если звуковая карта работает не на 48 кГц (например 44.1 или 96 кГц),
                            // ресэмплируем декодированный звук строго под частоту звуковой карты!
                            if sample_rate != 48000 {
                                let resampled = resample_linear(&mono_decoded[..len], 48000, sample_rate, &mut resample_pos);
                                pcm_buffer.extend_from_slice(&resampled);
                            } else {
                                pcm_buffer.extend_from_slice(&mono_decoded[..len]);
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