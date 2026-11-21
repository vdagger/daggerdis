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

/// Индекс устройства с таким именем в списке.
///
/// Выбор устройства живёт по имени: индексы cpal «плывут» между запусками и
/// сдвигаются, когда карту подключили или выдернули, а имя остаётся прежним.
pub fn index_of(devices: &[String], name: &str) -> Option<usize> {
    devices.iter().position(|d| d == name)
}

/// Снимок списка звуковых устройств на один момент времени.
///
/// `input_channels` параллелен `inputs` (числи каналов у каждого входа, 0 —
/// определить не удалось). `default_*` — имя устройства по умолчанию: список
/// мог сдвинуться, и откатываться нужно на актуальное умолчание, а не на
/// индекс 0 (это может быть выдернутая карта).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceSnapshot {
    pub inputs: Vec<String>,
    pub input_channels: Vec<u32>,
    pub outputs: Vec<String>,
    pub default_input: Option<String>,
    pub default_output: Option<String>,
}

impl DeviceSnapshot {
    /// Перечисляет устройства заново. Это ввод-вывод в драйверы (WASAPI на
    /// Windows перебирает конечные точки), поэтому в кадре GUI его не зовут —
    /// только из фонового обхода.
    pub fn scan() -> Self {
        let host = cpal::default_host();
        let default_input = host.default_input_device().and_then(|d| d.name().ok());
        let default_output = host.default_output_device().and_then(|d| d.name().ok());

        let (inputs, input_channels) = match host.input_devices() {
            Ok(devices) => devices
                .map(|device| {
                    let name = device
                        .name()
                        .unwrap_or_else(|_| "Unknown Input".to_string());
                    let channels = device
                        .default_input_config()
                        .map(|c| c.channels() as u32)
                        .unwrap_or(0);
                    (name, channels)
                })
                .unzip(),
            Err(_) => (Vec::new(), Vec::new()),
        };

        let outputs = match host.output_devices() {
            Ok(devices) => devices
                .map(|device| {
                    device
                        .name()
                        .unwrap_or_else(|_| "Unknown Output".to_string())
                })
                .collect(),
            Err(_) => Vec::new(),
        };

        Self {
            inputs,
            input_channels,
            outputs,
            default_input,
            default_output,
        }
    }
}

/// Какие устройства появились и какие пропали между двумя снимками.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceChange {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

impl DeviceChange {
    pub fn between(prev: &[String], next: &[String]) -> Self {
        let added = next
            .iter()
            .filter(|name| !prev.iter().any(|old| old == *name))
            .cloned()
            .collect();
        let removed = prev
            .iter()
            .filter(|name| !next.iter().any(|new| new == *name))
            .cloned()
            .collect();
        Self { added, removed }
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

/// Фоновый обход устройств: раз в `INTERVAL` снимает список и отдаёт его в
/// канал.
///
/// Зачем отдельный поток: перечисление — это запросы к драйверам, в кадре GUI
/// они дают рывок, а проверять подключение нужно и когда настройки закрыты
/// (наушники выдёргивают посреди разговора). Канал неограниченный; GUI за кадр
/// вычитывает всё накопившееся и берёт последний снимок, поэтому очередь не
/// растёт, пока окно живо и отрисовывается.
pub struct DeviceWatcher {
    rx: std::sync::mpsc::Receiver<DeviceSnapshot>,
}

impl DeviceWatcher {
    /// Две секунды — компромисс: USB-карта видна в списке через заметное, но
    /// не раздражающее время, а запросов к драйверам всё равно немного.
    pub const INTERVAL: std::time::Duration = std::time::Duration::from_millis(2000);

    pub fn spawn() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("albus-devices".to_string())
            .spawn(move || loop {
                let snapshot = DeviceSnapshot::scan();
                if tx.send(snapshot).is_err() {
                    // GUI закрылось — обход больше не нужен.
                    break;
                }
                std::thread::sleep(Self::INTERVAL);
            });
        if let Err(e) = spawned {
            eprintln!("[АУДИО] Обход устройств не запустился: {}", e);
        } else {
            println!(
                "[АУДИО] Обход устройств запущен: проверка раз в {} с",
                Self::INTERVAL.as_secs_f32()
            );
        }
        Self { rx }
    }

    /// Последний снимок, если обход успел его положить. Старые снимки в
    /// очереди не копятся: канал без буфера, а GUI забирает по одному за кадр.
    pub fn try_recv(&self) -> Option<DeviceSnapshot> {
        let mut latest = None;
        while let Ok(snapshot) = self.rx.try_recv() {
            latest = Some(snapshot);
        }
        latest
    }
}

/// Куда переехал выбор устройства после обновления списка.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reselection {
    /// Устройство на месте, просто стоит под новым индексом.
    Same(usize),
    /// Выбранного устройства в списке больше нет.
    Gone(Option<usize>),
}

/// Пересчитывает выбор по имени в свежем списке.
///
/// `default` — имя устройства по умолчанию: на него откатываемся, если
/// выбранного больше нет (индекс 0 может указывать на что угодно, в том числе
/// на выдернутую карту). `None` у `Gone` — откатываться некуда.
pub fn reselect(list: &[String], wanted: Option<&str>, default: Option<&str>) -> Reselection {
    let fallback = default.and_then(|name| index_of(list, name));
    match wanted {
        Some(name) => match index_of(list, name) {
            Some(index) => Reselection::Same(index),
            None => Reselection::Gone(fallback),
        },
        None => Reselection::Gone(fallback),
    }
}

/// Что сделать с выбором устройств после свежего снимка списка.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevicePlan {
    /// Снимок, который нужно принять вместо текущих списков.
    pub next: DeviceSnapshot,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Выбранные устройства, которых не стало (после отката).
    pub gone: Vec<String>,
    pub input: Reselection,
    pub input2: Reselection,
    pub output: Reselection,
}

/// Сводит свежий снимок с текущим выбором.
///
/// `None` — снимок принимать нельзя: драйвер во время перезагрузки карты
/// отдаёт пустые списки, и откат на «по умолчанию» порвал бы рабочий звук.
/// Иначе возвращается план: какие устройства появились/пропали, куда переехал
/// каждый выбор и куда откатиться, если выбранного больше нет.
#[allow(clippy::too_many_arguments)]
pub fn plan_device_update(
    inputs: &[String],
    outputs: &[String],
    selected_input: usize,
    selected_input2: usize,
    selected_output: usize,
    next: DeviceSnapshot,
) -> Option<DevicePlan> {
    if next.inputs.is_empty() || next.outputs.is_empty() {
        return None;
    }

    let wanted_in = inputs.get(selected_input).cloned();
    let wanted_in2 = if selected_input2 == 0 {
        None
    } else {
        inputs.get(selected_input2 - 1).cloned()
    };
    let wanted_out = outputs.get(selected_output).cloned();

    let input_change = DeviceChange::between(inputs, &next.inputs);
    let output_change = DeviceChange::between(outputs, &next.outputs);
    let mut added = input_change.added;
    added.extend(output_change.added);
    let mut removed = input_change.removed;
    removed.extend(output_change.removed);

    let mut gone = Vec::new();

    let input = match wanted_in {
        Some(name) => match reselect(&next.inputs, Some(&name), next.default_input.as_deref()) {
            Reselection::Same(index) => Reselection::Same(index),
            fallback => {
                gone.push(name);
                fallback
            }
        },
        None => Reselection::Gone(None),
    };

    // Второй вход не откатываем на умолчание: в инструмент молча пошёл бы
    // микрофон. «То же устройство» (0) — честный откат.
    let input2 = match wanted_in2 {
        Some(name) => match reselect(&next.inputs, Some(&name), None) {
            Reselection::Same(index) => Reselection::Same(index),
            fallback => {
                gone.push(name);
                fallback
            }
        },
        None => Reselection::Gone(None),
    };

    let output = match wanted_out {
        Some(name) => match reselect(&next.outputs, Some(&name), next.default_output.as_deref()) {
            Reselection::Same(index) => Reselection::Same(index),
            fallback => {
                gone.push(name);
                fallback
            }
        },
        None => Reselection::Gone(None),
    };

    Some(DevicePlan {
        next,
        added,
        removed,
        gone,
        input,
        input2,
        output,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn change_reports_added_and_removed() {
        let prev = names(&["Микрофон", "QUAD-CAPTURE", "Стереомикшер"]);
        let next = names(&["Микрофон", "USB Audio"]);
        let change = DeviceChange::between(&prev, &next);
        assert_eq!(change.added, names(&["USB Audio"]));
        assert_eq!(change.removed, names(&["QUAD-CAPTURE", "Стереомикшер"]));
        assert!(!change.is_empty());
    }

    #[test]
    fn change_is_empty_for_reordered_list() {
        // Порядок cpal не гарантирован, но набор тот же: это не повод
        // перезапускать потоки и пугать тостом.
        let prev = names(&["Микрофон", "QUAD-CAPTURE"]);
        let next = names(&["QUAD-CAPTURE", "Микрофон"]);
        assert!(DeviceChange::between(&prev, &next).is_empty());
    }

    #[test]
    fn change_of_empty_previous_lists_everything() {
        let change = DeviceChange::between(&[], &names(&["Микрофон"]));
        assert_eq!(change.added, names(&["Микрофон"]));
        assert!(change.removed.is_empty());
    }

    #[test]
    fn index_of_finds_by_name_after_shift() {
        // Карту выдернули: индексы сдвинулись, выбор должен остаться на имени.
        let before = names(&["USB Audio", "Микрофон"]);
        let after = names(&["Микрофон"]);
        let chosen = before[0].clone();
        assert_eq!(index_of(&before, &chosen), Some(0));
        assert_eq!(index_of(&after, &chosen), None);
        assert_eq!(index_of(&after, "Микрофон"), Some(0));
    }

    #[test]
    fn reselect_keeps_device_when_list_shifts() {
        let before = names(&["USB Audio", "Микрофон"]);
        let after = names(&["Микрофон", "USB Audio"]);
        assert_eq!(reselect(&after, Some(&before[0]), None), Reselection::Same(1));
    }

    #[test]
    fn reselect_falls_back_to_default_when_device_is_gone() {
        let list = names(&["Динамики (Realtek)", "QUAD-CAPTURE"]);
        assert_eq!(
            reselect(&list, Some("USB-наушники"), Some("QUAD-CAPTURE")),
            Reselection::Gone(Some(1))
        );
    }

    #[test]
    fn reselect_without_default_says_there_is_nowhere_to_go() {
        // Второй вход: молча на «устройство по умолчанию» не откатываемся.
        let list = names(&["Динамики (Realtek)"]);
        assert_eq!(
            reselect(&list, Some("Гитарный интерфейс"), None),
            Reselection::Gone(None)
        );
    }

    #[test]
    fn reselect_with_empty_default_uses_zero() {
        let list = names(&["Динамики (Realtek)"]);
        // Умолчания нет в списке — вызывающий код решает за индекс 0.
        assert_eq!(
            reselect(&list, Some("Пропавшее"), Some("Нет такого")),
            Reselection::Gone(None)
        );
    }

    fn snapshot(inputs: &[&str], outputs: &[&str]) -> DeviceSnapshot {
        DeviceSnapshot {
            input_channels: vec![2; inputs.len()],
            inputs: names(inputs),
            outputs: names(outputs),
            default_input: inputs.first().map(|name| name.to_string()),
            default_output: outputs.first().map(|name| name.to_string()),
        }
    }

    #[test]
    fn plan_keeps_selection_when_list_is_reordered() {
        let old_in = names(&["USB Audio", "QUAD-CAPTURE"]);
        let old_out = names(&["Динамики"]);
        let next = snapshot(&["QUAD-CAPTURE", "USB Audio"], &["Динамики"]);
        let plan = plan_device_update(&old_in, &old_out, 0, 0, 0, next).expect("план");

        // Выбран USB Audio — он остался, но теперь под индексом 1.
        assert_eq!(plan.input, Reselection::Same(1));
        assert!(plan.gone.is_empty(), "устройство не пропадало");
        assert!(plan.added.is_empty() && plan.removed.is_empty());
    }

    #[test]
    fn plan_falls_back_when_selected_input_disappears() {
        let old_in = names(&["USB Audio", "QUAD-CAPTURE"]);
        let old_out = names(&["Динамики"]);
        let next = snapshot(&["QUAD-CAPTURE"], &["Динамики"]);
        let plan = plan_device_update(&old_in, &old_out, 0, 0, 0, next).expect("план");

        assert_eq!(plan.gone, names(&["USB Audio"]));
        // Откат на устройство по умолчанию из снимка, а не на индекс 0.
        assert_eq!(plan.input, Reselection::Gone(Some(0)));
        assert_eq!(plan.removed, names(&["USB Audio"]));
    }

    #[test]
    fn plan_drops_second_input_to_same_device() {
        let old_in = names(&["Микрофон", "Гитара"]);
        let old_out = names(&["Динамики"]);
        // Второй вход выбран как «Гитара» — индекс 2 (n+1).
        let next = snapshot(&["Микрофон"], &["Динамики"]);
        let plan = plan_device_update(&old_in, &old_out, 0, 2, 0, next).expect("план");

        assert_eq!(plan.gone, names(&["Гитара"]));
        assert_eq!(plan.input2, Reselection::Gone(None));
    }

    #[test]
    fn plan_moves_output_to_default_when_it_disappears() {
        let old_in = names(&["Микрофон"]);
        let old_out = names(&["Наушники USB", "QUAD-CAPTURE"]);
        let next = snapshot(&["Микрофон"], &["QUAD-CAPTURE"]);
        let plan = plan_device_update(&old_in, &old_out, 0, 0, 0, next).expect("план");

        assert_eq!(plan.gone, names(&["Наушники USB"]));
        assert_eq!(plan.output, Reselection::Gone(Some(0)));
    }

    #[test]
    fn plan_ignores_empty_snapshot() {
        // Карта перезагружается: cpal на миг не видит ничего. Сбрасывать выбор
        // и рвать потоки из-за этого нельзя.
        let old_in = names(&["Микрофон"]);
        let old_out = names(&["Динамики"]);
        assert!(plan_device_update(&old_in, &old_out, 0, 0, 0, snapshot(&[], &[])).is_none());
    }

    #[test]
    fn plan_reports_new_device_without_touching_selection() {
        let old_in = names(&["Микрофон"]);
        let old_out = names(&["Динамики"]);
        let next = snapshot(&["Микрофон", "Микрофон USB"], &["Динамики"]);
        let plan = plan_device_update(&old_in, &old_out, 0, 0, 0, next).expect("план");

        assert_eq!(plan.added, names(&["Микрофон USB"]));
        assert!(plan.gone.is_empty());
        assert_eq!(plan.input, Reselection::Same(0));
    }
}
