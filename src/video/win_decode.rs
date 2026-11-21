//! Аппаратный H.264-декодер Windows (Media Foundation MFT).
//!
//! Вход — Annex-B кадры, выход — NV12 в системной памяти. Дальше
//! `win_d3d::Converter` переводит NV12 в BGRA на GPU для показа.
//!
//! Штатный декодер Microsoft — синхронный MFT (без IMFMediaEventGenerator),
//! аппаратные — асинхронные, поэтому поддерживаются оба режима.

use std::mem::ManuallyDrop;
use std::time::{Duration, Instant};

use windows::core::Interface;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED};

use super::publisher::StreamInfo;
use super::EncodedUnit;

const WAIT: Duration = Duration::from_secs(2);

enum Ev {
    NeedInput,
    HaveOutput,
    Other,
}

/// Результат одной попытки забрать выходной кадр.
enum Output {
    NeedInput,
    StreamChanged,
    Frame(Vec<u8>),
}

pub struct MfDecoder {
    transform: IMFTransform,
    /// `None` — MFT синхронный, работаем через ProcessInput/ProcessOutput.
    events: Option<IMFMediaEventGenerator>,
    output_info: MFT_OUTPUT_STREAM_INFO,
    frame_duration: i64,
    next_pts: i64,
    need_input: bool,
    width: u32,
    height: u32,
    fps: u32,
    /// Ширина строки NV12 на выходе декодера. Декодер выравнивает ширину
    /// (1108 → 1120), и читать его кадр как плотный нельзя: строки уезжают
    /// на (stride − width) байт за кадр, картинка идёт диагональными полосами.
    /// Берём из `MF_MT_DEFAULT_STRIDE`, а если его нет — считаем из размера.
    stride: Option<u32>,
    pub mft_name: String,
    pub notes: Vec<String>,
}

impl MfDecoder {
    pub fn new(info: &StreamInfo) -> Result<Self, String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            MFStartup(MF_VERSION, 0).map_err(|e| format!("MFStartup: {e}"))?;

            let activations = enumerate_h264_decoders()?;
            if activations.is_empty() {
                return Err("не найден H.264-декодер".into());
            }

            let mut last_err = String::new();
            for act in &activations {
                let name = activation_name(act).unwrap_or_else(|| "MFT".into());
                match Self::configure(act, info, name) {
                    Ok(dec) => return Ok(dec),
                    Err(e) => last_err = e,
                }
            }
            Err(format!("ни один декодер не настроился: {last_err}"))
        }
    }

    unsafe fn configure(
        act: &IMFActivate,
        info: &StreamInfo,
        name: String,
    ) -> Result<Self, String> {
        let transform: IMFTransform = act
            .ActivateObject()
            .map_err(|e| format!("{name}: ActivateObject: {e}"))?;

        let attrs = transform
            .GetAttributes()
            .map_err(|e| format!("{name}: GetAttributes: {e}"))?;
        attrs
            .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
            .map_err(|e| format!("{name}: async unlock: {e}"))?;
        attrs
            .SetUINT32(&MF_LOW_LATENCY, 1)
            .map_err(|e| format!("{name}: low latency: {e}"))?;

        let in_type = MFCreateMediaType().map_err(|e| e.to_string())?;
        in_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| e.to_string())?;
        in_type
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264)
            .map_err(|e| e.to_string())?;
        in_type
            .SetUINT64(&MF_MT_FRAME_SIZE, pack2(info.width, info.height))
            .map_err(|e| e.to_string())?;
        in_type
            .SetUINT64(&MF_MT_FRAME_RATE, pack2(info.fps, 1))
            .map_err(|e| e.to_string())?;
        in_type
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack2(1, 1))
            .map_err(|e| e.to_string())?;
        in_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|e| e.to_string())?;
        // SPS/PPS: декодер должен знать их до первого кадра
        if !info.sequence_header.is_empty() {
            in_type
                .SetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &info.sequence_header)
                .map_err(|e| format!("{name}: sequence header: {e}"))?;
        }
        transform
            .SetInputType(0, &in_type, 0)
            .map_err(|e| format!("{name}: SetInputType H264: {e}"))?;

        let out_type = MFCreateMediaType().map_err(|e| e.to_string())?;
        out_type
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
            .map_err(|e| e.to_string())?;
        out_type
            .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)
            .map_err(|e| e.to_string())?;
        out_type
            .SetUINT64(&MF_MT_FRAME_SIZE, pack2(info.width, info.height))
            .map_err(|e| e.to_string())?;
        out_type
            .SetUINT64(&MF_MT_FRAME_RATE, pack2(info.fps, 1))
            .map_err(|e| e.to_string())?;
        out_type
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack2(1, 1))
            .map_err(|e| e.to_string())?;
        out_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(|e| e.to_string())?;
        transform
            .SetOutputType(0, &out_type, 0)
            .map_err(|e| format!("{name}: SetOutputType NV12: {e}"))?;

        let mut notes = Vec::new();
        let events: Option<IMFMediaEventGenerator> = match transform.cast() {
            Ok(g) => Some(g),
            Err(_) => {
                notes.push("синхронный MFT".into());
                None
            }
        };
        let output_info = transform
            .GetOutputStreamInfo(0)
            .map_err(|e| format!("{name}: GetOutputStreamInfo: {e}"))?;
        // Шаг строки на выходе: у Microsoft-декодера ширина выравнивается до
        // 16 (1108 → 1120), и без этого атрибута кадр читается со сдвигом.
        let stride = transform
            .GetOutputCurrentType(0)
            .ok()
            .and_then(|t| t.GetUINT32(&MF_MT_DEFAULT_STRIDE).ok())
            .map(|v| (v as i32).unsigned_abs());

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
            frame_duration: 10_000_000 / info.fps.max(1) as i64,
            next_pts: 0,
            need_input: false,
            width: info.width,
            height: info.height,
            fps: info.fps,
            stride,
            mft_name: name,
            notes,
        })
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Ширина строки NV12 на выходе (диагностика и проба).
    pub fn output_stride(&self) -> Option<u32> {
        self.stride
    }

    /// Отдаёт один сжатый кадр, возвращает все декодированные NV12-кадры.
    pub fn push(&mut self, unit: &EncodedUnit) -> Result<Vec<Vec<u8>>, String> {
        if self.events.is_some() {
            self.push_async(unit)
        } else {
            self.push_sync(unit)
        }
    }

    fn push_async(&mut self, unit: &EncodedUnit) -> Result<Vec<Vec<u8>>, String> {
        let mut out = Vec::new();
        let deadline = Instant::now() + WAIT;

        while !self.need_input {
            match self.wait_event(deadline)? {
                Ev::NeedInput => self.need_input = true,
                Ev::HaveOutput => {
                    if let Output::Frame(b) = self.process_output_once()? {
                        out.push(b);
                    }
                }
                Ev::Other => {}
            }
        }
        self.send_input(unit)?;
        self.need_input = false;

        loop {
            match self.poll_event()? {
                Some(Ev::HaveOutput) => {
                    if let Output::Frame(b) = self.process_output_once()? {
                        out.push(b);
                    }
                }
                Some(Ev::NeedInput) => self.need_input = true,
                Some(Ev::Other) => {}
                None => break,
            }
        }
        Ok(out)
    }

    /// Синхронный MFT: вход может отклоняться, пока не вычитан выход.
    fn push_sync(&mut self, unit: &EncodedUnit) -> Result<Vec<Vec<u8>>, String> {
        let mut out = Vec::new();
        let sample = self.make_sample(unit)?;
        let mut pending = Some(sample);
        let deadline = Instant::now() + WAIT;

        loop {
            if let Some(s) = pending.take() {
                match unsafe { self.transform.ProcessInput(0, &s, 0) } {
                    Ok(()) => {}
                    Err(e) if e.code() == MF_E_NOTACCEPTING => pending = Some(s),
                    Err(e) => return Err(format!("ProcessInput: {e}")),
                }
            }
            loop {
                match self.process_output_once()? {
                    Output::Frame(b) => out.push(b),
                    Output::StreamChanged => continue,
                    Output::NeedInput => break,
                }
            }
            if pending.is_none() {
                break;
            }
            if Instant::now() >= deadline {
                return Err("декодер не принимает кадр (нет окна вывода)".into());
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(out)
    }

    fn poll_event(&self) -> Result<Option<Ev>, String> {
        let Some(events) = &self.events else {
            return Ok(None);
        };
        unsafe {
            match events.GetEvent(MF_EVENT_FLAG_NO_WAIT) {
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
                return Err("таймаут ожидания события декодера".into());
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

    fn make_sample(&mut self, unit: &EncodedUnit) -> Result<IMFSample, String> {
        unsafe {
            let buf = MFCreateMemoryBuffer(unit.data.len() as u32).map_err(|e| e.to_string())?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut current = 0u32;
            buf.Lock(&mut ptr, None, Some(&mut current))
                .map_err(|e| format!("Lock input: {e}"))?;
            std::ptr::copy_nonoverlapping(unit.data.as_ptr(), ptr, unit.data.len());
            buf.Unlock().map_err(|e| e.to_string())?;
            buf.SetCurrentLength(unit.data.len() as u32)
                .map_err(|e| e.to_string())?;

            // метки времени должны строго возрастать
            let pts = unit.pts_100ns.max(self.next_pts);
            self.next_pts = pts + self.frame_duration;

            let sample = MFCreateSample().map_err(|e| e.to_string())?;
            sample.AddBuffer(&buf).map_err(|e| e.to_string())?;
            sample.SetSampleTime(pts).map_err(|e| e.to_string())?;
            sample
                .SetSampleDuration(self.frame_duration)
                .map_err(|e| e.to_string())?;
            Ok(sample)
        }
    }

    fn send_input(&mut self, unit: &EncodedUnit) -> Result<(), String> {
        let sample = self.make_sample(unit)?;
        unsafe {
            self.transform
                .ProcessInput(0, &sample, 0)
                .map_err(|e| format!("ProcessInput: {e}"))
        }
    }

    fn process_output_once(&mut self) -> Result<Output, String> {
        unsafe {
            // Свежий сэмпл на каждый вызов. Переиспользовать один нельзя:
            // декодер Microsoft отвечает `CopyDecodedFrame failed (0x80004005)`.
            let provides_samples =
                self.output_info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32) != 0;
            let sample = if provides_samples {
                None
            } else {
                let size = self
                    .output_info
                    .cbSize
                    .max(self.width * self.height * 3 / 2);
                let buf = MFCreateMemoryBuffer(size).map_err(|e| e.to_string())?;
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
            let result = self
                .transform
                .ProcessOutput(0, std::slice::from_mut(&mut data), &mut status);

            // Сэмпл в `pSample` — в `ManuallyDrop`, чтобы его не тронул drop
            // структуры. Забираем его всегда, до разбора результата: иначе на
            // штатной ветке NEED_MORE_INPUT (конец каждого кадра) выделенный
            // под выход буфер NV12 (3 МБ на 1080p) оставался висеть — память
            // росла на кадр, ~180 МБ/с при 60 fps.
            let sample = ManuallyDrop::take(&mut data.pSample);

            match result {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => {
                    return Ok(Output::NeedInput)
                }
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    self.apply_stream_change()?;
                    return Ok(Output::StreamChanged);
                }
                Err(e) => return Err(format!("ProcessOutput: {e}")),
            }

            let sample = match sample {
                Some(s) => s,
                None => return Ok(Output::NeedInput),
            };
            let buf = sample
                .ConvertToContiguousBuffer()
                .map_err(|e| e.to_string())?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut current = 0u32;
            buf.Lock(&mut ptr, None, Some(&mut current))
                .map_err(|e| format!("Lock output: {e}"))?;
            let raw = std::slice::from_raw_parts(ptr, current as usize).to_vec();
            buf.Unlock().map_err(|e| e.to_string())?;
            Ok(Output::Frame(repack_nv12(raw, self.width, self.height, self.stride)))
        }
    }

    fn apply_stream_change(&mut self) -> Result<(), String> {
        unsafe {
            let mut index = 0;
            loop {
                let Ok(t) = self.transform.GetOutputAvailableType(0, index) else {
                    break;
                };
                let _ = t.SetUINT64(&MF_MT_FRAME_SIZE, pack2(self.width, self.height));
                let _ = t.SetUINT64(&MF_MT_FRAME_RATE, pack2(self.fps, 1));
                if self.transform.SetOutputType(0, &t, 0).is_ok() {
                    break;
                }
                index += 1;
            }
            self.output_info = self
                .transform
                .GetOutputStreamInfo(0)
                .map_err(|e| e.to_string())?;
            // Ширина строки могла измениться вместе с типом.
            self.stride = self
                .transform
                .GetOutputCurrentType(0)
                .ok()
                .and_then(|t| t.GetUINT32(&MF_MT_DEFAULT_STRIDE).ok())
                .map(|v| (v as i32).unsigned_abs());
            Ok(())
        }
    }
}

unsafe fn enumerate_h264_decoders() -> Result<Vec<IMFActivate>, String> {
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };

    let mut result = Vec::new();
    for flags in [
        MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER,
        MFT_ENUM_FLAG_SORTANDFILTER,
        MFT_ENUM_FLAG_ALL,
    ] {
        let mut raw: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut count = 0u32;
        if MFTEnumEx(
            MFT_CATEGORY_VIDEO_DECODER,
            flags,
            Some(&input),
            Some(&output),
            &mut raw,
            &mut count,
        )
        .is_err()
        {
            continue;
        }
        if raw.is_null() || count == 0 {
            continue;
        }
        for i in 0..count as usize {
            if let Some(act) = (*raw.add(i)).take() {
                result.push(act);
            }
        }
        CoTaskMemFree(Some(raw as *const core::ffi::c_void));
        if !result.is_empty() {
            break;
        }
    }
    Ok(result)
}

unsafe fn activation_name(act: &IMFActivate) -> Option<String> {
    let mut buf = [0u16; 256];
    let mut len = buf.len() as u32;
    act.GetString(&MFT_FRIENDLY_NAME_Attribute, &mut buf, Some(&mut len))
        .ok()?;
    let end = buf.iter().position(|&c| c == 0).unwrap_or(len as usize);
    Some(String::from_utf16_lossy(&buf[..end.min(buf.len())]))
}

/// Декодер отдаёт NV12 не плотным: высота выровнена по 16 (1088 вместо 1080),
/// а ширина — по 16 самим декодером (1108 → 1120). Плотный кадр нужен и
/// `upload_nv12`, и проверке длины, поэтому разбираем буфер по шагу строки, а
/// не «как есть»: иначе каждая строка уезжает на (шаг − ширина) байт и
/// картинка идёт диагональными полосами (видно на окнах с шириной не кратной
/// 16 — 1108x610).
///
/// `stride` — ширина строки с выхода (`MF_MT_DEFAULT_STRIDE`): сам декодер
/// знает её точно. Если атрибута нет, шаг выводится из размера буфера.
fn repack_nv12(data: Vec<u8>, w: u32, h: u32, stride: Option<u32>) -> Vec<u8> {
    let w = w as usize;
    let h = h as usize;
    let need = w * h * 3 / 2;
    let hinted = stride.map(|s| s as usize).filter(|s| *s >= w);
    // Уже плотный кадр: самая частая ветка (ширина кратна выравниванию).
    if data.len() == need && hinted.map(|s| s == w).unwrap_or(true) {
        return data;
    }
    let pitch = hinted.unwrap_or_else(|| guess_pitch(data.len(), w, h));
    let rows = if pitch >= w { data.len() / pitch } else { 0 };
    // Шаг из атрибута может не совпасть с фактическим буфером (декодер отдал
    // плотный кадр, но шаг в типе выровнен) — тогда читаем как плотный.
    let (pitch, rows) = if pitch >= w && rows >= h + h / 2 {
        (pitch, rows)
    } else {
        let p = guess_pitch(data.len(), w, h);
        if p >= w {
            (p, data.len() / p)
        } else {
            let mut v = data;
            v.resize(need, 0);
            v.truncate(need);
            return v;
        }
    };
    // Высота Y-плоскости в буфере: буфер = шаг × (h' + h'/2).
    let aligned_h = rows * 2 / 3;
    let mut out = vec![0u8; need];
    let copy_rows = |dst: &mut [u8], src_row: usize, count: usize, dst_off: usize| {
        for y in 0..count {
            let src = (src_row + y) * pitch;
            if src + w > data.len() || dst_off + y * w + w > dst.len() {
                break;
            }
            dst[dst_off + y * w..dst_off + y * w + w]
                .copy_from_slice(&data[src..src + w]);
        }
    };
    copy_rows(&mut out, 0, h, 0);
    copy_rows(&mut out, aligned_h, h / 2, w * h);
    out
}

/// Шаг строки по размеру буфера, если декодер не сказал его прямо:
/// ищем наименьшее выравнивание, при котором буфер раскладывается в NV12.
fn guess_pitch(len: usize, w: usize, h: usize) -> usize {
    if len == w * h * 3 / 2 {
        return w;
    }
    // Выровнена только высота: буфер = w × (h' + h'/2).
    if len.is_multiple_of(w) {
        let rows = len / w;
        if rows.is_multiple_of(3) && rows * 2 / 3 >= h {
            return w;
        }
    }
    for align in [16usize, 32, 64, 128, 256] {
        let p = w.next_multiple_of(align);
        if p < w || !len.is_multiple_of(p) {
            continue;
        }
        let rows = len / p;
        // rows = h' + h'/2 = h'·3/2, то есть делится на 3.
        if rows.is_multiple_of(3) && rows * 2 / 3 >= h {
            return p;
        }
    }
    0
}

fn pack2(a: u32, b: u32) -> u64 {
    ((a as u64) << 32) | b as u64
}

#[cfg(test)]
mod tests {
    use super::repack_nv12;

    /// Кадр 14x4 в буфере с шагом 16: Y — по строкам `y*10 + x`, UV — 128 полос,
    /// чтобы сдвиг шага был виден и по цвету.
    fn padded_buffer() -> Vec<u8> {
        let (pitch, h) = (16usize, 4usize);
        let mut buf = vec![0u8; pitch * (h + h / 2)];
        for y in 0..h {
            for x in 0..14 {
                buf[y * pitch + x] = (y * 10 + x) as u8;
            }
        }
        for y in 0..h / 2 {
            for x in 0..14 {
                buf[(h + y) * pitch + x] = 100 + y as u8;
            }
        }
        buf
    }

    #[test]
    fn repack_uses_decoder_stride_not_width() {
        // Именно этот случай давал диагональные полосы: 14 при выравнивании 16.
        let out = repack_nv12(padded_buffer(), 14, 4, Some(16));
        assert_eq!(out.len(), 14 * 4 * 3 / 2);
        for y in 0..4 {
            for x in 0..14 {
                assert_eq!(out[y * 14 + x], (y * 10 + x) as u8, "Y ({x},{y})");
            }
        }
        for y in 0..2 {
            for x in 0..14 {
                assert_eq!(
                    out[14 * 4 + y * 14 + x],
                    100 + y as u8,
                    "UV ({x},{y})"
                );
            }
        }
    }

    #[test]
    fn repack_guesses_stride_without_attribute() {
        let out = repack_nv12(padded_buffer(), 14, 4, None);
        assert_eq!(out[1], 1, "строка читается со своим шагом");
        assert_eq!(out[14 + 3], 13);
        assert_eq!(out[2 * 14 + 5], 25);
    }

    #[test]
    fn repack_keeps_packed_frame() {
        // Плотный кадр 1920x1080 возвращается байт в байт, без копий.
        let buf = vec![7u8; 1920 * 1080 * 3 / 2];
        let out = repack_nv12(buf.clone(), 1920, 1080, Some(1920));
        assert_eq!(out, buf);
        let out = repack_nv12(buf.clone(), 1920, 1080, None);
        assert_eq!(out, buf);
    }

    #[test]
    fn repack_handles_height_padding_without_stride() {
        // Декодер выровнял только высоту: 4 → 8 строк Y.
        let (w, h, pitch) = (14usize, 4usize, 14usize);
        let mut buf = vec![0u8; pitch * (8 + 4)];
        for y in 0..4 {
            for x in 0..w {
                buf[y * pitch + x] = (y * 10 + x) as u8;
            }
        }
        for y in 0..2 {
            for x in 0..w {
                buf[(8 + y) * pitch + x] = 200 + y as u8;
            }
        }
        let out = repack_nv12(buf, w as u32, h as u32, None);
        assert_eq!(out.len(), w * h * 3 / 2);
        assert_eq!(out[2 * w + 1], 21);
        assert_eq!(out[w * h + w], 201, "UV начинается ниже выровненной высоты");
    }
}
