//! Управление аудиопотоками

use cpal::traits::{DeviceTrait, HostTrait};
use cpal::Stream;
use std::sync::{Arc, Mutex};

/// Перезапускает входной аудиопоток для указанных слотов шины.
///
/// Новый поток собирается до остановки старого: если устройство занято
/// (например, гитарный интерфейс держит DAW в эксклюзивном режиме) или
/// выдернуто, рабочий вход остаётся живым, а ошибка уходит в настройки.
/// Для второго входа на «микрофон по умолчанию» не откатываемся — это молча
/// записало бы в инструмент не тот источник.
pub fn restart_audio_stream(
    stream_slot: &Arc<Mutex<Option<Stream>>>,
    mut audio: crate::audio::UniversalAudioCapture,
    tx: crate::txbus::TxContext,
    fill: [bool; 2],
    device_name: Option<&str>,
    allow_default_fallback: bool,
) -> Result<(), String> {
    let host = cpal::default_host();
    let mut device = None;
    if let Some(name) = device_name {
        device = host
            .input_devices()
            .ok()
            .and_then(|mut devs| devs.find(|d| d.name().unwrap_or_default() == name));
        if device.is_none() {
            if !allow_default_fallback {
                return Err(format!("устройство не найдено: {}", name));
            }
            eprintln!(
                "[АУДИО] Выбранное устройство не найдено: {} — берём микрофон по умолчанию",
                name
            );
        }
    }

    // Эпоха подготавливается заранее, а применяется только после успешной
    // сборки: неудачная попытка не должна оставлять без звука старый поток.
    let epochs = tx.bus.next_epochs(fill);
    let new_stream = audio
        .create_input_stream(device, tx.clone(), fill, epochs)
        .map_err(|e| e.to_string())?;
    tx.bus.commit_epochs(fill, epochs);

    let mut stream_lock = match stream_slot.lock() {
        Ok(l) => l,
        Err(p) => p.into_inner(),
    };
    *stream_lock = Some(new_stream);
    println!("[GUI] Вход успешно перезапущен.");
    Ok(())
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
