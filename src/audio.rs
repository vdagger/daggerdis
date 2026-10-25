use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub struct UniversalAudioCapture;

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

        let config = input_device.default_input_config()?;
        let total_channels = config.channels() as usize;
        let stream_config = config.config();

        println!(
            "[АУДИО] Открыт микрофон: '{}', частота: {} Гц, каналов: {}",
            input_device.name().unwrap_or_default(),
            stream_config.sample_rate.0,
            total_channels
        );

        // Создаем Opus-энкодер ОДИН РАЗ перед запуском потока (стандарт VoIP: 48кГц, моно)
        let mut encoder = rusty_opus::OpusEncoder::new(48000, 1, rusty_opus::Application::Voip)?;
        let mut sample_acc: Vec<f32> = Vec::with_capacity(1920);
        let mut mono_chunk: Vec<f32> = Vec::with_capacity(960);

        let stream = input_device.build_input_stream(
            &stream_config,
            move |data: &[f32], _| {
                mono_chunk.clear();
                // Настоящая звуковая волна: усредняем каналы без искажения фазы и знака
                for chunk in data.chunks_exact(total_channels) {
                    let sample = if total_channels > 1 {
                        (chunk[0] + chunk[1]) * 0.5
                    } else {
                        chunk[0]
                    };
                    mono_chunk.push(sample);
                }

                // Обработка громкости и VU-метра через микшер
                mixer.process_input(&mut mono_chunk);

                // Нарезаем сэмплы ровно по 960 (20 мс — золотой стандарт Opus VoIP)
                if ptt_flag.load(Ordering::Relaxed) {
                    sample_acc.extend_from_slice(&mono_chunk);
                    while sample_acc.len() >= 960 {
                        let frame: Vec<f32> = sample_acc.drain(..960).collect();
                        let mut packet_buf = vec![0u8; 1024];
                        if let Ok(encoded_len) = encoder.encode(&frame, 960, &mut packet_buf) {
                            let _ = packet_tx.try_send(packet_buf[..encoded_len].to_vec());
                        }
                    }
                    if sample_acc.len() > 4800 {
                        sample_acc.clear(); // Защита от накопления сетевой задержки
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
        rx: std::sync::Arc<std::sync::Mutex<tokio::sync::mpsc::Receiver<Vec<u8>>>>,
    ) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let output_device = match device {
            Some(d) => d,
            None => host.default_output_device().ok_or("No default output device")?,
        };

        let config = output_device.default_output_config()?.config();
        let output_channels = config.channels as usize;

        println!(
            "[АУДИО] Открыты динамики: '{}', частота: {} Гц, каналов: {}",
            output_device.name().unwrap_or_default(),
            config.sample_rate.0,
            output_channels
        );

        let mut decoder = rusty_opus::OpusDecoder::new(48000, 1)?;
        let mut mono_decoded = [0.0f32; 960];
        let mut pcm_buffer: Vec<f32> = Vec::with_capacity(3840);
        let mut prebuffered = false;

        let stream = output_device.build_output_stream(
            &config,
            move |data: &mut [f32], _| {
                // Выбираем все пришедшие пакеты из сетевой очереди
                if let Ok(mut rx_lock) = rx.try_lock() {
                    while let Ok(packet) = rx_lock.try_recv() {
                        if let Ok(len) = decoder.decode(&packet, 960, &mut mono_decoded) {
                            pcm_buffer.extend_from_slice(&mono_decoded[..len]);
                        }
                    }
                }

                let num_frames = data.len() / output_channels;

                // Джиттер-буфер: ждем накопления хотя бы 2 пакетов (40 мс) для плавного старта
                if !prebuffered {
                    if pcm_buffer.len() >= 960 * 2 {
                        prebuffered = true;
                    } else {
                        data.fill(0.0);
                        return;
                    }
                }

                if pcm_buffer.is_empty() {
                    prebuffered = false; // Буфер иссяк — мягко ждем следующий пакет
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
