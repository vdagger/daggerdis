use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub struct UniversalAudioCapture {
    sample_rate: u32,
    channels: u16,
    mono_buffer: Vec<f32>,
}

impl UniversalAudioCapture {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let sample_rate = 48000;
        let channels = 1;
        
        Ok(UniversalAudioCapture {
            sample_rate,
            channels,
            mono_buffer: Vec::with_capacity(1024),
        })
    }

    pub fn create_input_stream(
        &mut self,
        ptt_flag: Arc<AtomicBool>,
        packet_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
        mixer: std::sync::Arc<crate::mixer::AudioMixer>,
    ) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
        // Используем default_host для простоты
        let host = cpal::default_host();
        let input_device = host.default_input_device().ok_or("No default input device")?;
        
        // Получаем родную конфигурацию устройства
        let config = input_device.default_input_config()?;
        let total_channels = config.channels() as usize;
        
        let stream_config = cpal::StreamConfig {
            channels: config.channels(),
            sample_rate: config.sample_rate(),
            buffer_size: cpal::BufferSize::Fixed(480),
        };

        let mut mono_buffer = Vec::with_capacity(1024);
        
        let stream = input_device.build_input_stream(
            &stream_config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                // Линейное сканирование всех каналов для нахождения активного
                let mut raw_peak = 0.0f32;
                mono_buffer.clear();
                
                // Сканируем все каналы в каждом фрейме
                for chunk in data.chunks_exact(total_channels) {
                    // Находим максимальную амплитуду среди всех каналов в фрейме
                    let max_amplitude = chunk
                        .iter()
                        .map(|&sample| sample.abs())
                        .fold(0.0f32, f32::max);
                    
                    // Пушим максимальную амплитуду в mono_buffer
                    mono_buffer.push(max_amplitude);
                    
                    // Обновляем пик
                    raw_peak = raw_peak.max(max_amplitude);
                }
                
                // Обработка через микшер
                mixer.process_input(&mut mono_buffer);
                
                // Кодирование в Opus если PTT активен
                if ptt_flag.load(Ordering::Relaxed) {
                    // Простое кодирование в моно (1 канал)
                    let mut encoder = rusty_opus::OpusEncoder::new(48000, 1, rusty_opus::Application::Voip).unwrap();
                    let mut packet_buf = vec![0u8; 1024];
                    let frame_size = mono_buffer.len();
                    
                    let encoded_len = encoder.encode(&mono_buffer, frame_size, &mut packet_buf).unwrap();
                    let packet = packet_buf[..encoded_len].to_vec();
                    let _ = packet_tx.blocking_send(packet);
                }
            },
            move |err| {
                eprintln!("Audio input error: {}", err);
            },
            Some(Duration::from_millis(100)),
        )?;

        stream.play()?;
        Ok(stream)
    }
    
    pub fn start_playback(&self, mixer: std::sync::Arc<crate::mixer::AudioMixer>) -> tokio::sync::mpsc::Sender<Vec<u8>> {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(100);
        let host = cpal::default_host();
        let device = host.default_output_device().expect("No output device");
        let config = device.default_output_config().unwrap().config();
        
        // Инициализируем Opus-декодер (48000 Гц, 1 канал)
        let mut decoder = rusty_opus::OpusDecoder::new(48000, 1).unwrap();
        
        std::thread::spawn(move || {
            let stream = device.build_output_stream(&config, move |data: &mut [f32], _| {
                if let Ok(Ok(packet)) = rx.try_recv().map(|p| decoder.decode(&p, 480, data)) {
                    // Плеер нативно воспроизводит декодированный f32-звук пиров через динамики
                } else {
                    data.fill(0.0); // Тишина, если пакетов нет
                }
            }, |e| eprintln!("Output error: {}", e), None).unwrap();
            // Удерживаем поток воспроизведения живым внутри треда
            loop { std::thread::sleep(std::time::Duration::from_millis(100)); }
        });
        tx
    }
}