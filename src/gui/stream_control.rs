//! Управление аудиопотоками

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::Stream;
use std::sync::{Arc, Mutex};

/// Перезапускает аудиопоток микрофона
pub fn restart_audio_stream(
    mic_stream: &Arc<Mutex<Option<Stream>>>,
    mut audio: crate::audio::UniversalAudioCapture,
    packet_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    ptt_flag: Arc<std::sync::atomic::AtomicBool>,
    mixer: Arc<crate::mixer::AudioMixer>,
    device_name: String,
) {
    let mut stream_lock = match mic_stream.lock() {
        Ok(l) => l,
        Err(p) => p.into_inner(),
    };
    *stream_lock = None; // Останавливаем старый поток

    // Находим устройство по имени. Если оно исчезло (USB-микрофон выдернули),
    // молча оставались без звука — теперь откатываемся на устройство по
    // умолчанию, а не оставляем пользователя без микрофона.
    let host = cpal::default_host();
    let device = match host.input_devices().ok().and_then(|mut devs| {
        devs.find(|d| d.name().unwrap_or_default() == device_name)
    }) {
        Some(d) => Some(d),
        None => {
            eprintln!(
                "[АУДИО] Выбранное устройство не найдено: {} — берём микрофон по умолчанию",
                device_name
            );
            None
        }
    };

    match audio.create_input_stream(device, ptt_flag, packet_tx, mixer) {
        Ok(new_stream) => {
            *stream_lock = Some(new_stream);
            println!("[GUI] Аудиопоток успешно перезапущен.");
        }
        Err(e) => eprintln!("[ОШИБКА] Не удалось перезапустить поток: {}", e),
    }
}

/// Перезапускает аудиопоток воспроизведения
pub fn restart_playback_stream(
    playback_stream: &std::sync::Arc<std::sync::Mutex<Option<cpal::Stream>>>,
    rx: std::sync::Arc<std::sync::Mutex<tokio::sync::mpsc::Receiver<(u64, u32, Vec<u8>)>>>,
    device_name: String,
    mixer: std::sync::Arc<crate::mixer::AudioMixer>,
) {
    let mut stream_lock = playback_stream.lock().unwrap();
    *stream_lock = None; // Останавливаем старый поток

    let host = cpal::default_host();
    let device = host.output_devices().ok().and_then(|mut devs| {
        use cpal::traits::DeviceTrait;
        devs.find(|d| d.name().unwrap_or_default() == device_name)
    });
    if device.is_none() {
        eprintln!(
            "[АУДИО] Выбранные динамики не найдены: {} — берём устройство по умолчанию",
            device_name
        );
    }
    match crate::audio::UniversalAudioCapture::create_playback_stream(device, rx, mixer) {
        Ok(new_stream) => {
            *stream_lock = Some(new_stream);
            println!("[GUI] Поток воспроизведения переключен на: {}", device_name);
        }
        Err(e) => eprintln!("[ОШИБКА] Не удалось переключить динамики: {}", e),
    }
}
