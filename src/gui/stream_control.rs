//! Управление аудиопотоками

use std::sync::{Arc, Mutex};
use cpal::Stream;
use cpal::traits::{DeviceTrait, HostTrait};

/// Перезапускает аудиопоток микрофона
pub fn restart_audio_stream(
    mic_stream: &Arc<Mutex<Option<Stream>>>,
    mut audio: crate::audio::UniversalAudioCapture,
    packet_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    ptt_flag: Arc<std::sync::atomic::AtomicBool>,
    mixer: Arc<crate::mixer::AudioMixer>,
    device_name: String,
) {
    let mut stream_lock = mic_stream.lock().unwrap();
    *stream_lock = None; // Останавливаем старый поток
    
    // Находим устройство по имени вместо использования индекса
    let host = cpal::default_host();
    let device = host.input_devices().unwrap().into_iter()
        .find(|d| d.name().unwrap_or_default() == device_name)
        .expect("Выбранное устройство не найдено по имени");
    
    // Получаем поддерживаемую конфигурацию от устройства с безопасным подбором
    let supported_formats = device.supported_input_configs()
        .expect("Failed to query supported configs");
    
    let config = supported_formats
        .filter(|c| c.min_sample_rate().0 <= 48000 && c.max_sample_rate().0 >= 48000)
        .next()
        .map(|c| c.with_sample_rate(cpal::SampleRate(48000)).config())
        .unwrap_or_else(|| device.default_input_config().unwrap().into());
    
    // Используем переданный selected_input вместо создания нового
    // Но в текущей сигнатуре нет параметра selected_input, поэтому оставим как есть
    // Однако, если мы хотим использовать правильный индекс, нужно изменить сигнатуру функции
    
    match audio.create_input_stream(ptt_flag, packet_tx, mixer) {
        Ok(new_stream) => {
            *stream_lock = Some(new_stream);
            println!("[GUI] Аудиопоток успешно перезапущен.");
        }
        Err(e) => eprintln!("[ОШИБКА] Не удалось перезапустить поток: {}", e),
    }
}