//! Аппаратный H.264-энкодер Windows (Media Foundation MFT).
//!
//! Конкретный MFT выбирает драйвер (NVENC / AMF / QSV), для нас это один
//! интерфейс `IMFTransform`. Вход — NV12; на этом шаге кадры передаются
//! через системную память, путь без копирования (DXGI-текстура) — следующим.

use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};

use windows::core::{Interface, GUID};
use windows::Win32::Foundation::VARIANT_BOOL;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};
use windows::Win32::System::Variant::{VARIANT, VT_BOOL, VT_UI4};

use super::{EncodedUnit, VideoTarget};

/// Сколько ждать события от MFT, прежде чем считать, что он завис.
const WAIT: Duration = Duration::from_secs(5);

/// eAVEncH264VProfile_High
const H264_PROFILE_HIGH: u32 = 100;

enum Ev {
    NeedInput,
    HaveOutput,
    Other,
}

/// Асинхронный MFT-энкодер H.264.
///
/// Аппаратные MFT всегда асинхронные: кадр отдаётся не из ProcessInput,
/// а отдельными событиями NeedInput / HaveOutput.
pub struct MfEncoder {
    transform: IMFTransform,
    events: IMFMediaEventGenerator,
    output_info: MFT_OUTPUT_STREAM_INFO,
    frame_duration: i64,
    next_pts: i64,
    need_input: bool,
    target: VideoTarget,
    /// SPS/PPS: зритель должен получить их до первого кадра.
    pub sequence_header: Vec<u8>,
    pub mft_name: String,
    /// Что не удалось применить из ICodecAPI (диагностика).
    pub notes: Vec<String>,
}

impl MfEncoder {
    pub fn new(target: &VideoTarget) -> Result<Self, String> {
        unsafe {
            // Media Foundation требует инициализированного COM в потоке
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, 0).map_err(|e| format!("MFStartup: {e}"))?;

            let activations = enumerate_h264_encoders()?;
            if activations.is_empty() {
                return Err("не найдено аппаратных H.264-энкодеров (NVENC/AMF/QSV)".into());
            }

            // На гибридных машинах MF перечисляет энкодеры обеих видеокарт.
            // Для 1440p60 предпочитаем дискретную NVIDIA, если она есть.
            let mut ordered: Vec<&IMFActivate> = activations.iter().collect();
            ordered.sort_by_key(|a| {
                let name = activation_name(a).unwrap_or_default().to_lowercase();
                if name.contains("nvidia") {
                    0
                } else {
                    1
                }
            });

            let mut names = Vec::new();
            let mut last_err = String::new();
            for act in ordered {
                let name = activation_name(act).unwrap_or_else(|| "MFT".into());
                names.push(name.clone());
                match Self::configure(act, target, name) {
                    Ok(enc) => return Ok(enc),
                    Err(e) => last_err = e,
                }
            }
            Err(format!(
                "ни один MFT не настроился {:?}: {}",
                names, last_err
            ))
        }
    }

    unsafe fn configure(
        act: &IMFActivate,
        target: &VideoTarget,
        name: String,
    ) -> Result<Self, String> {
        let transform: IMFTransform = act
            .ActivateObject()
            .map_err(|e| format!("{name}: ActivateObject: {e}"))?;

        // Асинхронный MFT не даёт настраивать себя, пока не разблокирован.
        let attrs = transform
            .GetAttributes()
            .map_err(|e| format!("{name}: GetAttributes: {e}"))?;
        attrs
            .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
            .map_err(|e| format!("{name}: async unlock: {e}"))?;
        attrs
            .SetUINT32(&MF_LOW_LATENCY, 1)
            .map_err(|e| format!("{name}: low latency: {e}"))?;

        // Выход описываем первым: аппаратные энкодеры от него отталкиваются
        let out_type = MFCreateMediaType().map_err(|e| format!("{name}: MFCreateMediaType: {e}"))?;
        out_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| e.to_string())?;
        out_type
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
            .map_err(|e| e.to_string())?;
        out_type
            .SetUINT32(&MF_MT_AVG_BITRATE, target.bitrate_bps)
            .map_err(|e| e.to_string())?;
        out_type
            .SetUINT64(&MF_MT_FRAME_SIZE, pack2(target.width, target.height))
            .map_err(|e| e.to_string())?;
        out_type
            .SetUINT64(&MF_MT_FRAME_RATE, pack2(target.fps, 1))
            .map_err(|e| e.to_string())?;
        out_type
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack2(1, 1))
            .map_err(|e| e.to_string())?;
        out_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|e| e.to_string())?;
        out_type
            .SetUINT32(&MF_MT_MPEG2_PROFILE, H264_PROFILE_HIGH)
            .map_err(|e| e.to_string())?;
        transform
            .SetOutputType(0, &out_type, 0)
            .map_err(|e| format!("{name}: SetOutputType H264: {e}"))?;

        let in_type = MFCreateMediaType().map_err(|e| e.to_string())?;
        in_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| e.to_string())?;
        in_type
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
            .map_err(|e| e.to_string())?;
        in_type
            .SetUINT64(&MF_MT_FRAME_SIZE, pack2(target.width, target.height))
            .map_err(|e| e.to_string())?;
        in_type
            .SetUINT64(&MF_MT_FRAME_RATE, pack2(target.fps, 1))
            .map_err(|e| e.to_string())?;
        in_type
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack2(1, 1))
            .map_err(|e| e.to_string())?;
        in_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|e| e.to_string())?;
        transform
            .SetInputType(0, &in_type, 0)
            .map_err(|e| format!("{name}: SetInputType NV12: {e}"))?;

        // Выходного типа мало: режим и целевой битрейт задаются через ICodecAPI
        let notes = apply_codec_api(&transform, target, &name);

        let sequence_header = transform
            .GetOutputCurrentType(0)
            .ok()
            .and_then(|t| read_blob(&t, &MF_MT_MPEG_SEQUENCE_HEADER))
            .unwrap_or_default();

        let events: IMFMediaEventGenerator = transform
            .cast()
            .map_err(|e| format!("{name}: MFT не асинхронный: {e}"))?;
        let output_info = transform
            .GetOutputStreamInfo(0)
            .map_err(|e| format!("{name}: GetOutputStreamInfo: {e}"))?;

        transform
            .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
            .map_err(|e| e.to_string())?;
        transform
            .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
            .map_err(|e| e.to_string())?;

        Ok(Self {
            transform,
            events,
            output_info,
            frame_duration: 10_000_000 / target.fps.max(1) as i64,
            next_pts: 0,
            need_input: false,
            target: *target,
            sequence_header,
            mft_name: name,
            notes,
        })
    }

    /// Отдаёт один кадр NV12. Возвращает кадры, которые энкодер успел выдать.
    pub fn push_nv12(&mut self, nv12: &[u8]) -> Result<Vec<EncodedUnit>, String> {
        let mut out = Vec::new();
        let deadline = Instant::now() + WAIT;

        while !self.need_input {
            match self.wait_event(deadline)? {
                Ev::NeedInput => self.need_input = true,
                Ev::HaveOutput => out.extend(self.pull_output()?),
                Ev::Other => {}
            }
        }
        self.send_input(nv12)?;
        self.need_input = false;

        // Забираем всё готовое, не блокируясь; событие NeedInput не теряем.
        loop {
            match self.poll_event()? {
                Some(Ev::HaveOutput) => out.extend(self.pull_output()?),
                Some(Ev::NeedInput) => self.need_input = true,
                Some(Ev::Other) => {}
                None => break,
            }
        }
        Ok(out)
    }

    /// Конец потока: энкодер должен выдать всё, что накопил.
    pub fn finish(&mut self) -> Result<Vec<EncodedUnit>, String> {
        unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)
                .map_err(|e| e.to_string())?;
        }
        let mut out = Vec::new();
        let deadline = Instant::now() + WAIT;
        loop {
            match self.poll_event()? {
                Some(Ev::HaveOutput) => out.extend(self.pull_output()?),
                Some(Ev::NeedInput) => break,
                Some(Ev::Other) => {}
                None => {
                    if Instant::now() >= deadline {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
        }
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
        Ok(out)
    }

    fn poll_event(&self) -> Result<Option<Ev>, String> {
        unsafe {
            match self.events.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
                Ok(ev) => Ok(Some(Self::classify(&ev)?)),
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => Ok(None),
                Err(e) => Err(format!("GetEvent: {e}")),
            }
        }
    }

    fn wait_event(&self, deadline: Instant) -> Result<Ev, String> {
        loop {
            if let Some(ev) = self.poll_event()? {
                return Ok(ev);
            }
            if Instant::now() >= deadline {
                return Err("таймаут ожидания события MFT".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn classify(ev: &IMFMediaEvent) -> Result<Ev, String> {
        let t = unsafe { ev.GetType() }.map_err(|e| format!("GetType: {e}"))? as u32;
        Ok(if t == METransformNeedInput.0 as u32 {
            Ev::NeedInput
        } else if t == METransformHaveOutput.0 as u32 {
            Ev::HaveOutput
        } else {
            Ev::Other
        })
    }

    fn send_input(&mut self, nv12: &[u8]) -> Result<(), String> {
        unsafe {
            let buf = MFCreateMemoryBuffer(nv12.len() as u32).map_err(|e| e.to_string())?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut current = 0u32;
            buf.Lock(&mut ptr, None, Some(&mut current))
                .map_err(|e| format!("Lock input: {e}"))?;
            std::ptr::copy_nonoverlapping(nv12.as_ptr(), ptr, nv12.len());
            buf.Unlock().map_err(|e| e.to_string())?;
            buf.SetCurrentLength(nv12.len() as u32)
                .map_err(|e| e.to_string())?;

            let sample = MFCreateSample().map_err(|e| e.to_string())?;
            sample.AddBuffer(&buf).map_err(|e| e.to_string())?;
            sample.SetSampleTime(self.next_pts).map_err(|e| e.to_string())?;
            sample
                .SetSampleDuration(self.frame_duration)
                .map_err(|e| e.to_string())?;
            self.next_pts += self.frame_duration;

            self.transform
                .ProcessInput(0, &sample, 0)
                .map_err(|e| format!("ProcessInput: {e}"))
        }
    }

    fn pull_output(&mut self) -> Result<Vec<EncodedUnit>, String> {
        unsafe {
            let provides_samples =
                self.output_info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32) != 0;
            let sample = if provides_samples {
                None
            } else {
                let buf = MFCreateMemoryBuffer(self.output_info.cbSize.max(64 * 1024))
                    .map_err(|e| e.to_string())?;
                let s = MFCreateSample().map_err(|e| e.to_string())?;
                s.AddBuffer(&buf).map_err(|e| e.to_string())?;
                Some(s)
            };

            let mut data = MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(sample),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            };
            let mut status = 0u32;
            match self
                .transform
                .ProcessOutput(0, std::slice::from_mut(&mut data), &mut status)
            {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(Vec::new()),
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    self.apply_stream_change()?;
                    return Ok(Vec::new());
                }
                Err(e) => return Err(format!("ProcessOutput: {e}")),
            }

            let sample = ManuallyDrop::take(&mut data.pSample);
            let sample = match sample {
                Some(s) => s,
                None => return Ok(Vec::new()),
            };
            let pts = sample.GetSampleTime().unwrap_or(-1);
            let buf = sample
                .ConvertToContiguousBuffer()
                .map_err(|e| e.to_string())?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut current = 0u32;
            buf.Lock(&mut ptr, None, Some(&mut current))
                .map_err(|e| format!("Lock output: {e}"))?;
            let bytes = std::slice::from_raw_parts(ptr, current as usize);
            let unit = EncodedUnit {
                keyframe: has_idr(bytes),
                data: bytes.to_vec(),
                pts_100ns: pts,
            };
            buf.Unlock().map_err(|e| e.to_string())?;
            Ok(vec![unit])
        }
    }

    /// Энкодер сменил выходной формат — забираем доступный тип, но
    /// возвращаем в него наши размер, частоту и битрейт: AMF-энкодер
    /// подставляет в новый тип дефолт 30 fps и считает rate control по нему.
    fn apply_stream_change(&mut self) -> Result<(), String> {
        unsafe {
            let mut index = 0;
            loop {
                let Ok(t) = self.transform.GetOutputAvailableType(0, index) else {
                    break;
                };
                let _ = t.SetUINT64(
                    &MF_MT_FRAME_SIZE,
                    pack2(self.target.width, self.target.height),
                );
                let _ = t.SetUINT64(&MF_MT_FRAME_RATE, pack2(self.target.fps, 1));
                let _ = t.SetUINT32(&MF_MT_AVG_BITRATE, self.target.bitrate_bps);
                if self.transform.SetOutputType(0, &t, 0).is_ok() {
                    if let Ok(cur) = self.transform.GetOutputCurrentType(0) {
                        if let Some(seq) = read_blob(&cur, &MF_MT_MPEG_SEQUENCE_HEADER) {
                            self.sequence_header = seq;
                        }
                    }
                    break;
                }
                index += 1;
            }
            self.output_info = self
                .transform
                .GetOutputStreamInfo(0)
                .map_err(|e| e.to_string())?;
        }
        // после смены типа параметры rate control могли сброситься
        let mut more = apply_codec_api(&self.transform, &self.target, &self.mft_name);
        self.notes.append(&mut more);
        Ok(())
    }
}

/// Пробует настроить каждый найденный энкодер и возвращает отчёт
/// «имя: OK / ошибка» — чтобы видеть, почему выбран не тот MFT.
pub fn diagnose(target: &VideoTarget) -> Vec<String> {
    let mut report = Vec::new();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if MFStartup(MF_VERSION, 0).is_err() {
            return vec!["MFStartup не удался".into()];
        }
    }
    unsafe {
        match enumerate_h264_encoders() {
            Ok(list) => {
                for act in list {
                    let name = activation_name(&act).unwrap_or_else(|| "MFT".into());
                    let line = match MfEncoder::configure(&act, target, name.clone()) {
                        Ok(enc) => {
                            let mut line = format!("{name}: OK");
                            if !enc.notes.is_empty() {
                                line.push_str(&format!(" (notes: {:?})", enc.notes));
                            }
                            line
                        }
                        Err(e) => format!("{name}: {e}"),
                    };
                    report.push(line);
                }
            }
            Err(e) => report.push(e),
        }
    }
    report
}

/// Имена всех аппаратных H.264-энкодеров (диагностика).
pub fn available_encoders() -> Result<Vec<String>, String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, 0).map_err(|e| format!("MFStartup: {e}"))?;
        Ok(enumerate_h264_encoders()?
            .iter()
            .map(|a| activation_name(a).unwrap_or_else(|| "MFT".into()))
            .collect())
    }
}

/// Режим сжатия, средний битрейт и GOP задаются не выходным типом,
/// а параметрами ICodecAPI: без них энкодер уходит в свой VBR.
/// Неподдерживаемые драйвером параметры пишем в notes, а не падаем.
fn apply_codec_api(transform: &IMFTransform, target: &VideoTarget, name: &str) -> Vec<String> {
    let mut notes = Vec::new();
    unsafe {
        let codec: ICodecAPI = match transform.cast() {
            Ok(c) => c,
            Err(_) => {
                notes.push("ICodecAPI недоступен".into());
                return notes;
            }
        };
        // eAVEncCommonRateControlMode_CBR = 0
        if let Err(e) = codec.SetValue(&CODECAPI_AVEncCommonRateControlMode, &var_ui4(0)) {
            notes.push(format!("rate control: {e}"));
        }
        let mean = mean_bitrate_for(name, target);
        if let Err(e) = codec.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &var_ui4(mean)) {
            notes.push(format!("mean bitrate: {e}"));
        }
        if let Err(e) = codec.SetValue(&CODECAPI_AVEncMPVGOPSize, &var_ui4(gop_frames(target))) {
            notes.push(format!("gop: {e}"));
        }
        if let Err(e) = codec.SetValue(&CODECAPI_AVEncCommonRealTime, &var_bool(true)) {
            notes.push(format!("realtime: {e}"));
        }
    }
    notes
}

/// AMF-энкодер AMD делит средний битрейт на 30 кадров и игнорирует
/// MF_MT_FRAME_RATE: при 60 fps поток выходит вдвое жирнее заданного.
/// Компенсируем, пока MFT не научится читать частоту кадров.
fn mean_bitrate_for(name: &str, target: &VideoTarget) -> u32 {
    if name.to_lowercase().contains("amd") {
        (target.bitrate_bps as f64 * 30.0 / target.fps.max(1) as f64) as u32
    } else {
        target.bitrate_bps
    }
}

fn gop_frames(target: &VideoTarget) -> u32 {
    target.fps.saturating_mul(target.keyframe_secs).max(1)
}

fn var_ui4(v: u32) -> VARIANT {
    unsafe {
        let mut x: VARIANT = std::mem::zeroed();
        let inner = &mut *x.Anonymous.Anonymous;
        inner.vt = VT_UI4;
        inner.Anonymous.ulVal = v;
        x
    }
}

fn var_bool(v: bool) -> VARIANT {
    unsafe {
        let mut x: VARIANT = std::mem::zeroed();
        let inner = &mut *x.Anonymous.Anonymous;
        inner.vt = VT_BOOL;
        inner.Anonymous.boolVal = VARIANT_BOOL(if v { -1 } else { 0 });
        x
    }
}

/// Перечисляет зарегистрированные аппаратные H.264-энкодеры.
unsafe fn enumerate_h264_encoders() -> Result<Vec<IMFActivate>, String> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let mut raw: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    MFTEnumEx(
        MFT_CATEGORY_VIDEO_ENCODER,
        MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
        Some(&input),
        Some(&output),
        &mut raw,
        &mut count,
    )
    .map_err(|e| format!("MFTEnumEx: {e}"))?;

    if raw.is_null() || count == 0 {
        return Ok(Vec::new());
    }
    let mut list = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        if let Some(act) = (*raw.add(i)).take() {
            list.push(act);
        }
    }
    // массив выделен Media Foundation, освобождаем через CoTaskMemFree
    CoTaskMemFree(Some(raw as *const core::ffi::c_void));
    Ok(list)
}

unsafe fn activation_name(act: &IMFActivate) -> Option<String> {
    let mut buf = [0u16; 256];
    let mut len = 0u32;
    act.GetString(&MFT_FRIENDLY_NAME_Attribute, &mut buf, Some(&mut len))
        .ok()?;
    let end = buf.iter().position(|&c| c == 0).unwrap_or(len as usize);
    Some(String::from_utf16_lossy(&buf[..end.min(buf.len())]))
}

fn read_blob(attrs: &IMFAttributes, key: &GUID) -> Option<Vec<u8>> {
    unsafe {
        let len = attrs.GetBlobSize(key).ok()?;
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        let mut got = len;
        attrs.GetBlob(key, &mut buf, Some(&mut got)).ok()?;
        buf.truncate(got as usize);
        Some(buf)
    }
}

/// Два 32-битных числа в одно 64-битное: так MF_MT_FRAME_SIZE и MF_MT_FRAME_RATE
/// упаковывают пару «ширина/высота» и «числитель/знаменатель».
fn pack2(a: u32, b: u32) -> u64 {
    ((a as u64) << 32) | b as u64
}

/// Есть ли в потоке IDR-кадр (NAL type 5, Annex-B).
fn has_idr(data: &[u8]) -> bool {
    let mut i = 0;
    while i + 3 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            if data[i + 3] & 0x1f == 5 {
                return true;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    false
}
