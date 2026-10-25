use std::sync::Arc;

/// Обрабатывает команды пользователя из консоли
pub fn handle_command(line: &str, mixer: &Arc<crate::mixer::AudioMixer>) {
    let line = line.trim();
    if !line.starts_with('/') {
        return;
    }

    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.is_empty() {
        return;
    }

    match parts[0] {
        "/mute" => {
            let muted = mixer.is_mic_muted();
            mixer.set_mic_muted(!muted);
            let status = if mixer.is_mic_muted() { "ВЫКЛ" } else { "ВКЛ" };
            println!("[МИКШЕР] Микрофон: {}", status);
        }
        "/vol" => {
            if parts.len() > 1 {
                if let Ok(val) = parts[1].parse::<u32>() {
                    mixer.set_output_volume(val);
                    println!("[МИКШЕР] Громкость собеседников: {}%", mixer.output_volume());
                }
            }
        }
        "/mic" => {
            if parts.len() > 1 {
                if let Ok(val) = parts[1].parse::<u32>() {
                    mixer.set_ch1_volume(val);
                    println!("[МИКШЕР] Чувствительность микрофона: {}%", mixer.ch1_volume());
                }
            }
        }
        "/devices" => {
            let input_devices = crate::devices::DeviceSelector::get_input_devices();
            let output_devices = crate::devices::DeviceSelector::get_output_devices();
            
            println!("Доступные входные устройства:");
            for (i, device) in input_devices.iter().enumerate() {
                println!("  {}. {}", i + 1, device);
            }
            
            println!("Доступные выходные устройства:");
            for (i, device) in output_devices.iter().enumerate() {
                println!("  {}. {}", i + 1, device);
            }
        }
        _ => {}
    }
}