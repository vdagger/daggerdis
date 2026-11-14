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

    /// Находит устройство ввода по имени (порядок устройств меняется между запусками)
    pub fn find_input_device_by_name(name: &str) -> Option<cpal::Device> {
        let host = cpal::default_host();
        let mut devices = host.input_devices().ok()?;
        devices.find(|d| d.name().map(|n| n == name).unwrap_or(false))
    }

    /// Находит устройство вывода по имени
    pub fn find_output_device_by_name(name: &str) -> Option<cpal::Device> {
        let host = cpal::default_host();
        let mut devices = host.output_devices().ok()?;
        devices.find(|d| d.name().map(|n| n == name).unwrap_or(false))
    }

    /// Индекс устройства ввода по умолчанию в списке get_input_devices()
    pub fn default_input_index() -> Option<usize> {
        let host = cpal::default_host();
        let default_name = host.default_input_device()?.name().ok()?;
        Self::get_input_devices()
            .iter()
            .position(|n| *n == default_name)
    }

    /// Индекс устройства вывода по умолчанию в списке get_output_devices()
    pub fn default_output_index() -> Option<usize> {
        let host = cpal::default_host();
        let default_name = host.default_output_device()?.name().ok()?;
        Self::get_output_devices()
            .iter()
            .position(|n| *n == default_name)
    }
}
