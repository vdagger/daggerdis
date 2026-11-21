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

/// Перезапускает аудиопоток воспроизведения.
///
/// Порядок как у входа: сначала собираем новый поток и только после успеха
/// отпускаем старый. Раньше старый обнулялся до сборки, и ошибка (устройство
/// занято, выдернуто, формат не принят) оставляла приложение совсем без звука
/// до перезапуска. На время сборки старый поток ставится на паузу, чтобы два
/// потока не растаскивали пакеты из одного приёмника.
pub fn restart_playback_stream(
    playback_stream: &std::sync::Arc<std::sync::Mutex<Option<cpal::Stream>>>,
    rx: std::sync::Arc<std::sync::Mutex<tokio::sync::mpsc::Receiver<(u64, u32, Vec<u8>)>>>,
    device_name: String,
    mixer: std::sync::Arc<crate::mixer::AudioMixer>,
) -> Result<(), String> {
    use cpal::traits::StreamTrait;

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

    // Замок держим на всё время сборки: иначе новый поток начнёт забирать
    // пакеты из общего приёмника, пока старый ещё играет.
    let mut stream_lock = match playback_stream.lock() {
        Ok(l) => l,
        Err(p) => p.into_inner(),
    };
    if let Some(old) = stream_lock.as_ref() {
        let _ = old.pause();
    }

    match crate::audio::UniversalAudioCapture::create_playback_stream(device, rx, mixer) {
        Ok(new_stream) => {
            *stream_lock = Some(new_stream);
            println!("[GUI] Поток воспроизведения переключен на: {}", device_name);
            Ok(())
        }
        Err(e) => {
            // Новый не собрался — возвращаем к жизни прежний поток.
            if let Some(old) = stream_lock.as_ref() {
                let _ = old.play();
            }
            let msg = format!("динамики «{}» не открылись: {}", device_name, e);
            eprintln!("[ОШИБКА] {}", msg);
            Err(msg)
        }
    }
}
