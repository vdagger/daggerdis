//! Модуль для работы с аудиоустройствами системы (микрофоны и динамики).

use cpal::traits::{DeviceTrait, HostTrait};

/// Структура для работы с аудиоустройствами
pub struct DeviceSelector;

impl DeviceSelector {
    /// Получает список всех доступных устройств ввода (микрофонов)
    pub fn get_input_devices() -> Vec<String> {
        let host = cpal::default_host();
        let devices = match host.input_devices() {
            Ok(devices) => devices,
            Err(_) => return Vec::new(),
        };

        devices
            .map(|device| {
                device
                    .name()
                    .unwrap_or_else(|_| "Unknown Input".to_string())
            })
            .collect()
    }

    /// Получает количество входных каналов для каждого устройства ввода
    /// (параллельно get_input_devices; 0 — если определить не удалось)
    pub fn get_input_channel_counts() -> Vec<u32> {
        let host = cpal::default_host();
        let devices = match host.input_devices() {
            Ok(devices) => devices,
            Err(_) => return Vec::new(),
        };

        devices
            .map(|device| {
                device
                    .default_input_config()
                    .map(|c| c.channels() as u32)
                    .unwrap_or(0)
            })
            .collect()
    }

    /// Получает список всех доступных устройств вывода (динамиков/наушников)
    pub fn get_output_devices() -> Vec<String> {
        let host = cpal::default_host();
        let devices = match host.output_devices() {
            Ok(devices) => devices,
            Err(_) => return Vec::new(),
        };

        devices
            .map(|device| {
                device
                    .name()
                    .unwrap_or_else(|_| "Unknown Output".to_string())
            })
            .collect()
    }

    /// Находит устройство ввода по его порядковому номеру
    pub fn find_input_device_by_index(index: usize) -> Option<cpal::Device> {
        let host = cpal::default_host();
        let devices = match host.input_devices() {
            Ok(devices) => devices,
            Err(_) => return None,
        };

        // Преобразуем в вектор, чтобы можно было использовать nth()
        let device_vec: Vec<_> = devices.collect();
        device_vec.get(index).cloned()
    }

    /// Находит устройство вывода по его порядковому номеру
    pub fn find_output_device_by_index(index: usize) -> Option<cpal::Device> {
        let host = cpal::default_host();
        let devices = match host.output_devices() {
            Ok(devices) => devices,
            Err(_) => return None,
        };

        // Преобразуем в вектор, чтобы можно было использовать nth()
        let device_vec: Vec<_> = devices.collect();
        device_vec.get(index).cloned()
    }
}
