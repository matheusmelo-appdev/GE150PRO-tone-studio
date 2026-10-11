use crate::device_commands as command;
use hidapi::{HidApi, HidDevice};
use serde::{Deserialize, Serialize};
use std::{
    error::Error,
    sync::{mpsc, OnceLock},
    thread,
    time::{Duration, Instant},
};

const VENDOR_ID: u16 = 0x0483;
const PRODUCT_ID: u16 = 0x5703;
const REPORT_PAYLOAD_MAX: usize = 0x3f;

struct HidSession {
    _api: HidApi,
    device: HidDevice,
}

impl HidSession {
    fn open() -> Result<Self, String> {
        let api = HidApi::new().map_err(|error| error.to_string())?;
        let info = api
            .device_list()
            .find(|device| device.vendor_id() == VENDOR_ID && device.product_id() == PRODUCT_ID)
            .ok_or("GE150 Pro USB device (VID 0483, PID 5703) not found")?;
        let device = info.open_device(&api).map_err(|error| error.to_string())?;
        Ok(Self { _api: api, device })
    }
}

type HidJob = Box<dyn FnOnce(&mut Option<HidSession>) + Send + 'static>;
static HID_WORKER: OnceLock<mpsc::Sender<HidJob>> = OnceLock::new();

fn run_on_hid_thread<T: Send + 'static>(
    task: impl FnOnce(&mut Option<HidSession>) -> T + Send + 'static,
) -> T {
    let (result_tx, result_rx) = mpsc::channel();
    let worker = HID_WORKER.get_or_init(|| {
        let (worker_tx, worker_rx) = mpsc::channel::<HidJob>();
        thread::Builder::new()
            .name("ge150-hid".into())
            .spawn(move || {
                let mut session = None;
                while let Ok(job) = worker_rx.recv() {
                    job(&mut session);
                }
            })
            .expect("could not start GE150 USB worker");
        worker_tx
    });
    worker
        .send(Box::new(move |session| {
            let _ = result_tx.send(task(session));
        }))
        .expect("GE150 USB worker stopped");
    result_rx.recv().expect("GE150 USB worker dropped a result")
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawModule {
    pub model_index: u16,
    pub enabled: bool,
    pub parameters: Vec<u16>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeviceFrame {
    pub command: u8,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicePreset {
    pub slot: u8,
    pub name: String,
    pub modules: Vec<Option<RawModule>>,
    pub exp_words: Vec<u16>,
    pub raw_data: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomModelNames {
    pub amplifiers: Vec<String>,
    pub cabinets: Vec<String>,
}

/// Boundary between preset logic and the physical USB transport.
pub trait PedalInterface: Send + Sync {
    fn read_presets(&self) -> Result<Vec<DevicePreset>, String>;
    fn activate(&self, slot: u8) -> Result<(), String>;
    fn update_module(&self, slot: usize, data: &[u8]) -> Result<(), String>;
    fn write_preset(&self, slot: u8, data: &[u8]) -> Result<(), String>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UsbPedal;

impl PedalInterface for UsbPedal {
    fn read_presets(&self) -> Result<Vec<DevicePreset>, String> {
        read_all_presets().map_err(|error| error.to_string())
    }

    fn activate(&self, slot: u8) -> Result<(), String> {
        activate_preset(slot)
    }

    fn update_module(&self, slot: usize, data: &[u8]) -> Result<(), String> {
        update_module(slot, data)
    }

    fn write_preset(&self, slot: u8, data: &[u8]) -> Result<(), String> {
        write_preset(slot, data)
    }
}

pub fn read_all_preset_frames() -> Result<Vec<DeviceFrame>, Box<dyn Error>> {
    request_frames(command::ALL_PRESETS)
}

pub fn read_all_presets() -> Result<Vec<DevicePreset>, Box<dyn Error>> {
    let presets = read_all_preset_frames()?
        .into_iter()
        .filter(|frame| frame.command == command::PRESET_RECORD)
        .filter_map(|frame| decode_preset_record(&frame.payload))
        .collect::<Vec<_>>();
    if presets.is_empty() {
        return Err("The pedal returned no preset records".into());
    }
    Ok(presets)
}

pub fn read_device_library() -> Result<(Vec<DevicePreset>, Option<CustomModelNames>), String> {
    run_on_hid_thread(|session| -> Result<_, String> {
        if session.is_none() {
            *session = Some(HidSession::open()?);
        }
        let device = &session.as_ref().expect("session opened").device;
        let result = (|| {
            write_request_frame(device, command::READ_DEVICE_SYSTEM_DATA)?;
            write_request_frame(device, command::ALL_PRESETS)?;
            read_response_frames_after_idle(device, 6, true).map_err(|error| error.to_string())
        })();
        if result.is_err() {
            *session = None;
        }
        result
    })
    .and_then(|frames| {
        let frames = frames
            .into_iter()
            .filter_map(|frame| {
                (frame.len() >= 6).then(|| DeviceFrame {
                    command: frame[4],
                    payload: frame[5..frame.len() - 2].to_vec(),
                })
            })
            .collect::<Vec<_>>();
        let presets = frames
            .iter()
            .filter(|frame| frame.command == command::PRESET_RECORD)
            .filter_map(|frame| decode_preset_record(&frame.payload))
            .collect::<Vec<_>>();
        if !has_complete_preset_library(&presets) {
            return Err(format!(
                "device sync incomplete: received {} of 200 unique preset slots",
                presets.len()
            ));
        }
        let names = frames
            .iter()
            .find(|frame| frame.command == command::CUSTOM_MODEL_NAMES_RESPONSE)
            .and_then(|frame| decode_custom_model_names(&frame.payload));
        Ok((presets, names))
    })
}

fn has_complete_preset_library(presets: &[DevicePreset]) -> bool {
    if presets.len() != 200 {
        return false;
    }
    let mut slots = [false; 200];
    for preset in presets {
        if !(1..=200).contains(&preset.slot) || slots[usize::from(preset.slot - 1)] {
            return false;
        }
        slots[usize::from(preset.slot - 1)] = true;
    }
    slots.into_iter().all(|slot| slot)
}

pub fn validate_gnr(data: &[u8]) -> Result<(), String> {
    gnr_samples(data).map(|_| ())
}

pub fn upload_custom_amp_gnr(slot: u8, data: Vec<u8>, name: String) -> Result<(), String> {
    let packets = build_amp_upload_packets(slot, &data, &name)?;
    upload_packets(slot, packets, true)
}

fn build_amp_upload_packets(slot: u8, data: &[u8], name: &str) -> Result<Vec<Vec<u8>>, String> {
    if slot >= 20 {
        return Err("custom AMP slot must be between 0 and 19".into());
    }
    let samples = gnr_samples(data)?;
    let name = upload_name(name)?;
    let target = slot;
    let mut metadata = Vec::with_capacity(18);
    metadata.extend_from_slice(&[target, 0x14]);
    metadata.extend_from_slice(&name);
    let mut packets = vec![build_command_frame(command::AMP_IMPORT_PACKET, &metadata)?];
    for (chunk, samples) in samples.chunks_exact(0x200).enumerate() {
        let mut payload = Vec::with_capacity(samples.len() + 2);
        payload.extend_from_slice(&[target, chunk as u8]);
        payload.extend_from_slice(samples);
        packets.push(build_command_frame(command::AMP_IMPORT_PACKET, &payload)?);
    }
    if packets.len() != 21 {
        return Err("GNR sample data must contain exactly 20 transfer blocks".into());
    }
    Ok(packets)
}

pub fn upload_custom_ir_wav(slot: u8, data: Vec<u8>, name: String) -> Result<(), String> {
    let packets = build_ir_upload_packets(slot, &data, &name)?;
    upload_packets(0x1a + slot, packets, false)
}

fn build_ir_upload_packets(slot: u8, data: &[u8], name: &str) -> Result<Vec<Vec<u8>>, String> {
    if slot >= 20 {
        return Err("custom CAB slot must be between 0 and 19".into());
    }
    let samples = wav_ir_samples(data)?;
    let name = upload_name(name)?;
    let target = slot;
    let mut packets = Vec::with_capacity(5);
    packets.push(build_command_frame(
        command::IR_IMPORT_PACKET,
        &[0, target, 0, 1],
    )?);
    let mut name_payload = Vec::with_capacity(17);
    name_payload.push(1);
    name_payload.extend_from_slice(&name);
    packets.push(build_command_frame(
        command::IR_IMPORT_PACKET,
        &name_payload,
    )?);
    for (index, chunk) in samples.chunks_exact(512).enumerate() {
        let mut payload = Vec::with_capacity(513);
        payload.push(index as u8 + 2);
        payload.extend_from_slice(chunk);
        packets.push(build_command_frame(command::IR_IMPORT_PACKET, &payload)?);
    }
    Ok(packets)
}

fn upload_packets(target: u8, packets: Vec<Vec<u8>>, wait_for_amp_ack: bool) -> Result<(), String> {
    run_on_hid_thread(move |session| {
        if session.is_none() {
            *session = Some(HidSession::open()?);
        }
        let device = &session.as_ref().expect("session opened").device;
        let result = (|| {
            for (index, packet) in packets.iter().enumerate() {
                write_hid_frame(device, packet)?;
                if wait_for_amp_ack {
                    let block = if index == 0 { 20 } else { index as u8 - 1 };
                    if let Err(error) = read_import_ack(device, target, block) {
                        return Err(if index == 0 {
                            format!("AMP name/metadata packet was not acknowledged; no sample blocks were sent, but the slot name may have changed. Close MOOER Studio and check the slot before retrying. {error}")
                        } else {
                            format!("AMP data block {block} was not acknowledged; the custom slot may be partially uploaded. Close MOOER Studio and check the slot before retrying. {error}")
                        });
                    }
                    thread::sleep(Duration::from_millis(100));
                } else {
                    thread::sleep(Duration::from_millis(100));
                }
            }
            Ok(())
        })();
        if result.is_err() {
            *session = None;
        }
        result
    })
}

fn read_import_ack(device: &HidDevice, target: u8, block: u8) -> Result<(), String> {
    let started = Instant::now();
    let mut stream = Vec::new();
    let mut responses = Vec::new();
    while started.elapsed() < Duration::from_secs(5) {
        let mut report = [0u8; 64];
        let count = device
            .read_timeout(&mut report, 100)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            continue;
        }
        let payload = if count == 64 && report[0] <= REPORT_PAYLOAD_MAX as u8 {
            &report[1..1 + usize::from(report[0])]
        } else {
            &report[..count]
        };
        stream.extend_from_slice(payload);
        for frame in parse_frames(&mut stream) {
            if import_ack_matches(&frame, target, block) {
                return Ok(());
            }
            if frame.len() >= 5 && responses.len() < 12 {
                let summary = if frame.get(4) == Some(&command::IMPORT_ACK) && frame.len() >= 7 {
                    format!("0x15[{:#04x},{}]", frame[5], frame[6])
                } else {
                    format!("0x{:02x}", frame[4])
                };
                if !responses.contains(&summary) {
                    responses.push(summary);
                }
            }
        }
    }
    Err(format!(
        "no matching 0x15 ACK for target {target}, block {block} after 5 seconds; observed {:?}",
        responses
    ))
}

fn import_ack_matches(frame: &[u8], target: u8, block: u8) -> bool {
    frame.get(4) == Some(&command::IMPORT_ACK)
        && frame.get(5) == Some(&target)
        && frame.get(6) == Some(&block)
}

fn gnr_samples(data: &[u8]) -> Result<&[u8], String> {
    if data.len() < 20
        || !(data.starts_with(b"mooerge\0") || data.starts_with(b"mooerst\0"))
        || u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize != data.len() - 12
    {
        return Err("not a valid MOOER GNR container".into());
    }
    let mut offset = 12usize;
    let mut has_info = false;
    while offset < data.len() {
        let header_end = offset
            .checked_add(8)
            .filter(|end| *end <= data.len())
            .ok_or("truncated GNR chunk header")?;
        let len = u32::from_le_bytes(data[offset + 4..header_end].try_into().unwrap()) as usize;
        let end = header_end
            .checked_add(len)
            .filter(|end| *end <= data.len())
            .ok_or("truncated GNR chunk data")?;
        match &data[offset..offset + 4] {
            b"info" if !has_info => has_info = true,
            b"data" if has_info && len == 0x2800 && end == data.len() => {
                return Ok(&data[header_end..end]);
            }
            _ => {}
        }
        offset = end;
    }
    Err("GNR file has no valid 10,240-byte AMP data chunk".into())
}

fn upload_name(name: &str) -> Result<[u8; 16], String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("custom model name cannot be empty".into());
    }
    let mut result = [0; 16];
    let mut len = name.len().min(result.len());
    while !name.is_char_boundary(len) {
        len -= 1;
    }
    result[..len].copy_from_slice(&name.as_bytes()[..len]);
    Ok(result)
}

// shortcut: WAV conversion linearly resamples to 44.1 kHz and 512 24-bit taps; verify against an official conversion capture before expanding support.
fn wav_ir_samples(data: &[u8]) -> Result<Vec<u8>, String> {
    const RATE: u32 = 44_100;
    const TAPS: usize = 512;
    if data.len() < 44 || &data[..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return Err("choose a RIFF/WAVE file".into());
    }
    let riff_end = (u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize)
        .checked_add(8)
        .filter(|end| *end <= data.len() && *end >= 12)
        .ok_or("invalid WAV RIFF length")?;
    let mut format = None;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut byte_rate = 0u32;
    let mut block_align = 0u16;
    let mut bits = 0u16;
    let mut samples = None;
    let mut offset = 12usize;
    while offset.checked_add(8).is_some_and(|end| end <= riff_end) {
        let id = &data[offset..offset + 4];
        let size = u32::from_le_bytes(data[offset + 4..offset + 8].try_into().unwrap()) as usize;
        let start = offset + 8;
        let end = start
            .checked_add(size)
            .filter(|end| *end <= riff_end)
            .ok_or("WAV chunk extends past end of file")?;
        if id == b"fmt " {
            if size < 16 {
                return Err("WAV fmt chunk is too short".into());
            }
            format = Some(u16::from_le_bytes(
                data[start..start + 2].try_into().unwrap(),
            ));
            channels = u16::from_le_bytes(data[start + 2..start + 4].try_into().unwrap());
            sample_rate = u32::from_le_bytes(data[start + 4..start + 8].try_into().unwrap());
            byte_rate = u32::from_le_bytes(data[start + 8..start + 12].try_into().unwrap());
            block_align = u16::from_le_bytes(data[start + 12..start + 14].try_into().unwrap());
            bits = u16::from_le_bytes(data[start + 14..start + 16].try_into().unwrap());
        } else if id == b"data" {
            samples = Some(&data[start..end]);
        }
        offset = end + (size & 1);
    }
    let format = format.ok_or("WAV file is missing fmt chunk")?;
    let samples = samples.ok_or("WAV file is missing data chunk")?;
    if format != 1
        || !matches!(channels, 1 | 2)
        || !matches!(bits, 16 | 24 | 32)
        || !matches!(sample_rate, 44_100 | 48_000 | 88_200 | 96_000)
    {
        return Err(
            "supported WAVs are PCM16/24/32, mono or stereo, at 44.1/48/88.2/96 kHz".into(),
        );
    }
    let bytes_per_sample = usize::from(bits / 8);
    let frame_bytes = bytes_per_sample * usize::from(channels);
    if usize::from(block_align) != frame_bytes
        || byte_rate != sample_rate.saturating_mul(frame_bytes as u32)
        || samples.len() % frame_bytes != 0
    {
        return Err("WAV data is not aligned to complete sample frames".into());
    }
    let frames = samples.len() / frame_bytes;
    if frames == 0 {
        return Err("WAV contains no samples".into());
    }
    let sample = |frame: usize, channel: usize| -> f64 {
        let at = frame * frame_bytes + channel * bytes_per_sample;
        match bits {
            16 => i16::from_le_bytes([samples[at], samples[at + 1]]) as f64 / 32768.0,
            24 => {
                let raw = (i32::from(samples[at + 2]) << 16)
                    | (i32::from(samples[at + 1]) << 8)
                    | i32::from(samples[at]);
                ((raw << 8) >> 8) as f64 / 8_388_608.0
            }
            _ => {
                i32::from_le_bytes(samples[at..at + 4].try_into().unwrap()) as f64 / 2_147_483_648.0
            }
        }
    };
    let output_frames = ((frames as u64 * u64::from(RATE)) / u64::from(sample_rate)) as usize;
    let mut output = Vec::with_capacity(TAPS * 3);
    for at in 0..TAPS {
        let position = at as f64 * f64::from(sample_rate) / f64::from(RATE);
        let left = (position.floor() as usize).min(frames - 1);
        let right = (left + 1).min(frames - 1);
        let fraction = position - position.floor();
        let value = if at >= output_frames {
            0.0
        } else {
            let channel = |index| {
                let a = sample(left, index);
                let b = sample(right, index);
                a + (b - a) * fraction
            };
            (0..usize::from(channels)).map(channel).sum::<f64>() / f64::from(channels)
        }
        .clamp(-0.999_999, 0.999_999);
        let bytes = ((value * 8_388_608.0).round() as i32).to_le_bytes();
        output.extend_from_slice(&bytes[..3]);
    }
    Ok(output)
}

fn decode_custom_model_names(payload: &[u8]) -> Option<CustomModelNames> {
    const SLOT_COUNT: usize = 20;
    const NAME_BYTES: usize = 16;
    const NAMES_BYTES: usize = SLOT_COUNT * NAME_BYTES;
    if payload.len() < NAMES_BYTES * 2 {
        return None;
    }
    let read_names = |start: usize| {
        (0..SLOT_COUNT)
            .map(|slot| {
                let at = start + slot * NAME_BYTES;
                String::from_utf8_lossy(&payload[at..at + NAME_BYTES])
                    .trim_matches(['\0', ' '])
                    .to_owned()
            })
            .collect()
    };
    Some(CustomModelNames {
        amplifiers: read_names(0),
        cabinets: read_names(NAMES_BYTES),
    })
}

pub fn activate_preset(slot: u8) -> Result<(), String> {
    if !(1..=200).contains(&slot) {
        return Err("GE150 preset slot must be between 1 and 200".into());
    }
    send_command(command::ACTIVATE_PRESET, &[slot])
}

pub fn update_module(slot: usize, module_data: &[u8]) -> Result<(), String> {
    if slot >= 9 || module_data.len() != 24 {
        return Err("invalid GE150 module update".into());
    }
    send_command(command::MODULE_UPDATE_BASE + slot as u8, module_data)
}

pub fn write_preset(slot: u8, raw_data: &[u8]) -> Result<(), String> {
    if !(1..=200).contains(&slot) || raw_data.len() != 244 {
        return Err("invalid GE150 preset write payload".into());
    }
    let mut payload = Vec::with_capacity(raw_data.len() + 1);
    payload.push(slot);
    payload.extend_from_slice(raw_data);
    send_command(command::WRITE_PRESET, &payload)
}

fn send_command(command: u8, payload: &[u8]) -> Result<(), String> {
    let frame = build_command_frame(command, payload)?;
    run_on_hid_thread(move |session| {
        if session.is_none() {
            *session = Some(HidSession::open()?);
        }
        let result = write_hid_frame(&session.as_ref().expect("session opened").device, &frame);
        if result.is_err() {
            *session = None;
        }
        result
    })
}

fn write_hid_frame(device: &HidDevice, frame: &[u8]) -> Result<(), String> {
    for chunk in frame.chunks(REPORT_PAYLOAD_MAX) {
        let report = hid_report(chunk);
        if device.write(&report).map_err(|error| error.to_string())? != report.len() {
            return Err("USB command was not fully sent".into());
        }
    }
    Ok(())
}

fn hid_report(chunk: &[u8]) -> [u8; 64] {
    let mut report = [0; 64];
    report[0] = chunk.len() as u8;
    report[1..1 + chunk.len()].copy_from_slice(chunk);
    report
}

fn write_request_frame(device: &HidDevice, command: u8) -> Result<(), String> {
    let request = request_frame(command, 1);
    write_hid_frame(device, &request)
}

fn build_command_frame(command: u8, payload: &[u8]) -> Result<Vec<u8>, String> {
    let length = payload
        .len()
        .checked_add(1)
        .filter(|length| *length <= u16::MAX as usize)
        .ok_or("USB command payload is too large")? as u16;
    let mut frame = Vec::with_capacity(usize::from(length) + 6);
    frame.extend_from_slice(&[0xaa, 0x55]);
    frame.extend_from_slice(&length.to_le_bytes());
    frame.push(command);
    frame.extend_from_slice(payload);
    let checksum = !crc16(&frame[2..]);
    frame.extend_from_slice(&checksum.to_be_bytes());
    Ok(frame)
}

fn decode_preset_record(payload: &[u8]) -> Option<DevicePreset> {
    const NAME_START: usize = 1;
    const NAME_LEN: usize = 16;
    const MODULES_START: usize = 17;
    const MODULE_BYTES: usize = 24;
    const EXP_START: usize = MODULES_START + 9 * MODULE_BYTES;
    if payload.len() < EXP_START + 12 {
        return None;
    }
    let name = String::from_utf8_lossy(&payload[NAME_START..NAME_START + NAME_LEN])
        .trim_matches(['\0', ' '])
        .to_owned();
    let modules = (0..9)
        .map(|slot| {
            let start = MODULES_START + slot * MODULE_BYTES;
            let data = &payload[start..start + MODULE_BYTES];
            let word = |at| u16::from_le_bytes([data[at], data[at + 1]]);
            Some(RawModule {
                enabled: word(0) != 0,
                model_index: word(2),
                parameters: (4..MODULE_BYTES).step_by(2).map(word).collect(),
            })
        })
        .collect();
    let exp_words = (EXP_START..EXP_START + 12)
        .step_by(2)
        .map(|at| u16::from_le_bytes([payload[at], payload[at + 1]]))
        .collect();
    Some(DevicePreset {
        slot: payload[0],
        name,
        modules,
        exp_words,
        raw_data: payload[1..].to_vec(),
    })
}

fn request_frames(command: u8) -> Result<Vec<DeviceFrame>, Box<dyn Error>> {
    request_frames_for_commands(vec![command])
}

fn request_frames_for_commands(commands: Vec<u8>) -> Result<Vec<DeviceFrame>, Box<dyn Error>> {
    run_on_hid_thread(move |session| {
        if session.is_none() {
            *session = Some(HidSession::open()?);
        }
        let result = request_frames_on_hid_thread(
            &session.as_ref().expect("session opened").device,
            commands,
        );
        if result.is_err() {
            *session = None;
        }
        result
    })
    .map_err(|error| std::io::Error::other(error).into())
}

fn request_frames_on_hid_thread(
    device: &HidDevice,
    commands: Vec<u8>,
) -> Result<Vec<DeviceFrame>, String> {
    for command in commands {
        write_request_frame(device, command)?;
    }

    Ok(read_response_frames(&device)
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter_map(|frame| {
            (frame.len() >= 6).then(|| DeviceFrame {
                command: frame[4],
                payload: frame[5..frame.len() - 2].to_vec(),
            })
        })
        .collect())
}

fn request_frame(command: u8, argument: u8) -> [u8; 8] {
    let mut frame = [0xaa, 0x55, 0x02, 0x00, command, argument, 0x00, 0x00];
    let checksum = !crc16(&frame[2..6]);
    frame[6..8].copy_from_slice(&checksum.to_be_bytes());
    frame
}

fn read_response_frames(device: &HidDevice) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
    read_response_frames_after_idle(device, 6, false)
}

fn read_response_frames_after_idle(
    device: &HidDevice,
    idle_read_limit: usize,
    request_custom_names_after_system_data: bool,
) -> Result<Vec<Vec<u8>>, Box<dyn Error>> {
    let started = Instant::now();
    let mut stream = Vec::new();
    let mut frames = Vec::new();
    let mut idle_reads = 0;
    let mut requested_custom_names = false;
    while started.elapsed() < Duration::from_secs(45) && idle_reads < idle_read_limit {
        let mut report = [0u8; 64];
        let count = device.read_timeout(&mut report, 250)?;
        if count == 0 {
            if !frames.is_empty() {
                idle_reads += 1;
            }
            continue;
        }
        idle_reads = 0;
        let payload = if usize::from(report[0]) <= REPORT_PAYLOAD_MAX {
            &report[1..count]
        } else {
            &report[..count]
        };
        stream.extend_from_slice(payload);
        for frame in parse_frames(&mut stream) {
            if request_custom_names_after_system_data
                && !requested_custom_names
                && frame.get(4) == Some(&command::DEVICE_SYSTEM_DATA_RESPONSE)
            {
                write_request_frame(device, command::READ_CUSTOM_MODEL_NAMES)
                    .map_err(std::io::Error::other)?;
                write_request_frame(device, command::READ_RHYTHM_DATA)
                    .map_err(std::io::Error::other)?;
                requested_custom_names = true;
            }
            frames.push(frame);
        }
    }
    Ok(frames)
}

fn parse_frames(stream: &mut Vec<u8>) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    loop {
        let Some(start) = stream.windows(2).position(|bytes| bytes == [0xaa, 0x55]) else {
            stream.clear();
            break;
        };
        if start > 0 {
            stream.drain(..start);
        }
        if stream.len() < 4 {
            break;
        }
        let data_len = usize::from(u16::from_le_bytes([stream[2], stream[3]]));
        if data_len > 2048 {
            stream.drain(..1);
            continue;
        }
        let frame_len = data_len + 6;
        if stream.len() < frame_len {
            break;
        }
        let checksum_at = frame_len - 2;
        let expected = u16::from_be_bytes([stream[checksum_at], stream[checksum_at + 1]]);
        let actual = !crc16(&stream[2..checksum_at]);
        if expected != actual {
            stream.drain(..1);
            continue;
        }
        frames.push(stream.drain(..frame_len).collect());
    }
    frames
}

fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0u16;
    for byte in bytes {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::{
        build_amp_upload_packets, build_command_frame, build_ir_upload_packets, crc16,
        decode_custom_model_names, decode_preset_record, gnr_samples, has_complete_preset_library,
        hid_report, import_ack_matches, parse_frames, request_frame, run_on_hid_thread,
        wav_ir_samples, DevicePreset,
    };
    use crate::device_commands as command;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn hid_tasks_reuse_one_thread() {
        let first = run_on_hid_thread(|_| std::thread::current().id());
        let second = run_on_hid_thread(|_| std::thread::current().id());
        assert_eq!(first, second);
    }

    #[test]
    fn custom_model_names_decode_amp_then_cab_slots() {
        let mut payload = vec![0; 20 * 16 * 2];
        payload[..16].copy_from_slice(b"P-Mesa Dual Rec ");
        payload[20 * 16..20 * 16 + 16].copy_from_slice(b"Studio IR       ");
        let names = decode_custom_model_names(&payload).unwrap();
        assert_eq!(names.amplifiers[0], "P-Mesa Dual Rec");
        assert_eq!(names.cabinets[0], "Studio IR");
        assert!(names.amplifiers[1].is_empty());
        assert!(decode_custom_model_names(&payload[..100]).is_none());
    }

    #[test]
    fn gnr_parser_and_amp_upload_match_official_blocks() {
        let source = test_gnr_file();
        let samples = gnr_samples(&source).unwrap();
        assert_eq!(samples.len(), 0x2800);

        let packets = build_amp_upload_packets(
            0,
            &source,
            "Hellwin SYN100-H Lead 2 (Knobs at Noon)",
        )
        .unwrap();
        assert_eq!(packets.len(), 21);
        assert_eq!(&packets[0][..7], &[0xaa, 0x55, 0x13, 0, 0xd3, 0, 0x14]);
        assert_eq!(&packets[0][7..23], b"Hellwin SYN100-H");
        assert_eq!(&packets[1][..7], &[0xaa, 0x55, 0x03, 0x02, 0xd3, 0, 0]);
        assert_eq!(&packets[1][7..519], &samples[..512]);
        assert_eq!(
            u16::from_be_bytes([packets[1][519], packets[1][520]]),
            !crc16(&packets[1][2..519])
        );
        assert!(gnr_samples(b"bad").is_err());
    }

    fn test_gnr_file() -> Vec<u8> {
        let samples = (0..0x2800).map(|index| index as u8).collect::<Vec<_>>();
        let mut data = Vec::with_capacity(12 + 64 + 8 + samples.len());
        data.extend_from_slice(b"mooerge\0");
        data.extend_from_slice(&((64 + 8 + samples.len()) as u32).to_le_bytes());
        data.extend_from_slice(b"info");
        data.extend_from_slice(&56u32.to_le_bytes());
        data.extend_from_slice(&[0; 56]);
        data.extend_from_slice(b"data");
        data.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        data.extend_from_slice(&samples);
        data
    }

    #[test]
    fn wav_parser_builds_512_mono_24bit_taps() {
        let rate = 44_100u32;
        let samples = vec![0, 0, 0x20].repeat(512);
        let mut wav = Vec::with_capacity(44 + samples.len());
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&((36 + samples.len()) as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 3).to_le_bytes());
        wav.extend_from_slice(&3u16.to_le_bytes());
        wav.extend_from_slice(&24u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(samples.len() as u32).to_le_bytes());
        wav.extend_from_slice(&samples);

        let output = wav_ir_samples(&wav).unwrap();
        assert_eq!(output.len(), 512 * 3);
        assert_eq!(&output[..3], &[0, 0, 0x20]);
        let packets = build_ir_upload_packets(0, &wav, "R5_412_V30_Edgy").unwrap();
        assert_eq!(packets.len(), 5);
        assert_eq!(&packets[0][..9], &[0xaa, 0x55, 5, 0, 0xb5, 0, 0, 0, 1]);
        assert_eq!(&packets[1][..6], &[0xaa, 0x55, 0x12, 0, 0xb5, 1]);
        assert_eq!(&packets[1][6..22], b"R5_412_V30_Edgy\0");
        assert_eq!(&packets[2][..6], &[0xaa, 0x55, 2, 2, 0xb5, 2]);
        assert_eq!(&packets[2][6..518], &output[..512]);
        assert!(wav_ir_samples(b"not wave").is_err());
    }

    #[test]
    fn current_preset_request_has_editor_frame_and_checksum() {
        let request = request_frame(command::ALL_PRESETS, 1);
        assert_eq!(&request[..6], &[0xaa, 0x55, 2, 0, command::ALL_PRESETS, 1]);
        assert_eq!(
            u16::from_be_bytes([request[6], request[7]]),
            !crc16(&request[2..6])
        );
    }

    #[test]
    fn custom_model_names_request_uses_editor_command_and_checksum() {
        let request = request_frame(command::READ_CUSTOM_MODEL_NAMES, 1);
        assert_eq!(
            &request[..6],
            &[0xaa, 0x55, 2, 0, command::READ_CUSTOM_MODEL_NAMES, 1]
        );
        assert_eq!(
            u16::from_be_bytes([request[6], request[7]]),
            !crc16(&request[2..6])
        );
    }

    #[test]
    fn hid_report_prefix_is_payload_length() {
        let request = request_frame(command::READ_CUSTOM_MODEL_NAMES, 1);
        let report = hid_report(&request);
        assert_eq!(report[0], request.len() as u8);
        assert_eq!(&report[1..1 + request.len()], &request);
        assert!(report[1 + request.len()..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn gnr_metadata_ack_uses_zero_based_slot_index() {
        let ack = build_command_frame(command::IMPORT_ACK, &[0, 20]).unwrap();
        assert!(import_ack_matches(&ack, 0, 20));
        assert!(!import_ack_matches(&ack, 0x37, 20));
        assert!(!import_ack_matches(&ack, 0, 0));
    }

    #[test]
    fn preset_write_packet_has_c3_target_and_payload_length() {
        let frame = build_command_frame(command::WRITE_PRESET, &[7; 245]).unwrap();
        assert_eq!(
            &frame[..6],
            &[0xaa, 0x55, 0xf6, 0x00, command::WRITE_PRESET, 7]
        );
        assert_eq!(frame.len(), 252);
        assert_eq!(
            u16::from_be_bytes([frame[250], frame[251]]),
            !crc16(&frame[2..250])
        );
    }

    #[test]
    fn preset_write_frame_has_slot_and_exact_raw_payload() {
        let frame = build_command_frame(command::WRITE_PRESET, &[7; 245]).unwrap();
        assert_eq!(
            &frame[..6],
            &[0xaa, 0x55, 0xf6, 0x00, command::WRITE_PRESET, 7]
        );
        assert_eq!(frame.len(), 252);
        assert_eq!(
            u16::from_be_bytes([frame[250], frame[251]]),
            !crc16(&frame[2..250])
        );
    }

    #[test]
    fn response_parser_resynchronizes_and_checks_crc() {
        let mut frame = vec![0xaa, 0x55, 0x02, 0x00, 0x02, 0x01];
        frame.extend_from_slice(&(!crc16(&frame[2..])).to_be_bytes());
        let mut stream = vec![0, 0xaa, 0, 0x55];
        stream.extend_from_slice(&frame);
        stream.extend_from_slice(&[0xaa, 0x55, 0xff, 0xff]);
        let frames = parse_frames(&mut stream);
        assert_eq!(frames, vec![frame]);
        assert!(stream.len() <= 3);
    }

    #[test]
    fn all_preset_record_decodes_slot_name_modules_and_exp_words() {
        let mut payload = vec![0; 245];
        payload[0] = 3;
        payload[1..17].copy_from_slice(b"SYN Rhythm      ");
        payload[17..19].copy_from_slice(&1u16.to_le_bytes());
        payload[19..21].copy_from_slice(&11u16.to_le_bytes());
        payload[21..23].copy_from_slice(&41u16.to_le_bytes());
        payload[233..245].copy_from_slice(&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]);
        let preset = decode_preset_record(&payload).unwrap();
        assert_eq!(preset.slot, 3);
        assert_eq!(preset.name, "SYN Rhythm");
        assert_eq!(preset.modules[0].as_ref().unwrap().model_index, 11);
        assert!(preset.modules[0].as_ref().unwrap().enabled);
        assert_eq!(preset.modules[0].as_ref().unwrap().parameters[0], 41);
        assert_eq!(preset.exp_words, vec![256, 770, 1284, 1798, 2312, 2826]);
    }

    #[test]
    fn startup_library_requires_all_200_unique_slots() {
        let complete = (1..=200)
            .map(|slot| DevicePreset {
                slot,
                name: String::new(),
                modules: Vec::new(),
                exp_words: Vec::new(),
                raw_data: Vec::new(),
            })
            .collect::<Vec<_>>();
        assert!(has_complete_preset_library(&complete));
        assert!(!has_complete_preset_library(&complete[..199]));
        let mut duplicate = complete;
        duplicate[199].slot = 1;
        assert!(!has_complete_preset_library(&duplicate));
    }

    #[test]
    #[ignore = "requires connected GE150 Pro with the MOOER editor closed"]
    fn reads_all_preset_frames_from_connected_pedal() {
        let presets = super::read_all_presets().unwrap();
        println!("received {} preset records", presets.len());
        for index in [0, 1, 198, 199, 200, 201, 398, 399] {
            if let Some(preset) = presets.get(index) {
                println!("{index}: slot={} {}", preset.slot, preset.name);
            }
        }
        assert!(presets.len() >= 200);
        assert_eq!(presets[2].name, "SYN Rhythm");
        let rhythm = &presets[2];
        assert_eq!(rhythm.modules[0].as_ref().unwrap().model_index, 11);
        assert!(!rhythm.modules[0].as_ref().unwrap().enabled);
        assert_eq!(rhythm.modules[0].as_ref().unwrap().parameters[0], 41);
        assert_eq!(
            rhythm.modules[1].as_ref().unwrap().parameters[..3],
            [85, 62, 10]
        );
        assert_eq!(
            rhythm.modules[2].as_ref().unwrap().parameters[..6],
            [44, 50, 45, 65, 60, 93]
        );
        assert_eq!(
            rhythm.modules[5].as_ref().unwrap().parameters[6..9],
            [398, 1997, 6771]
        );
    }

    #[test]
    #[ignore = "read-only hardware test; requires connected GE150 Pro with MOOER Studio closed"]
    fn reads_custom_amp_and_ir_names_from_connected_pedal() {
        let frames = run_on_hid_thread(|session| -> Result<Vec<Vec<u8>>, String> {
            if session.is_none() {
                *session = Some(super::HidSession::open()?);
            }
            let device = &session.as_ref().unwrap().device;
            let send_read = |cmd| {
                let request = request_frame(cmd, 1);
                super::write_hid_frame(device, &request)
            };

            send_read(command::READ_DEVICE_SYSTEM_DATA)?;
            send_read(command::ALL_PRESETS)?;
            let frames = super::read_response_frames_after_idle(device, 40, true)
                .map_err(|error| error.to_string())?;
            let system_replies = frames
                .iter()
                .filter_map(|frame| (frame.len() > 4).then_some(format!("0x{:02X}", frame[4])))
                .collect::<BTreeSet<_>>();
            println!("Replies after system-data + preset requests: {system_replies:?}");
            if !system_replies.contains(&format!("0x{:02X}", command::DEVICE_SYSTEM_DATA_RESPONSE))
            {
                return Err("pedal did not return system-data response 0x32".into());
            }

            Ok(frames)
        })
        .unwrap();

        let replies = frames
            .iter()
            .filter_map(|frame| (frame.len() >= 6).then_some(format!("0x{:02X}", frame[4])))
            .collect::<BTreeSet<_>>();
        println!("Replies after 0x32 -> 0xC1 -> 0x8C: {replies:?}");
        let commands = frames
            .iter()
            .filter_map(|frame| (frame.len() >= 6).then_some(frame[4]))
            .collect::<BTreeSet<_>>();
        println!(
            "Response commands: {:?}",
            commands
                .iter()
                .map(|command| format!("0x{command:02X}"))
                .collect::<Vec<_>>()
        );
        assert!(commands.contains(&command::CUSTOM_MODEL_STATUS_RESPONSE));
        assert!(commands.contains(&command::RHYTHM_DATA_RESPONSE));
        if let Some(names) = frames
            .iter()
            .find(|frame| frame.get(4) == Some(&command::CUSTOM_MODEL_NAMES_RESPONSE))
            .and_then(|frame| decode_custom_model_names(&frame[5..frame.len() - 2]))
        {
            println!(
                "AMP names: {:?}\nIR names: {:?}",
                names.amplifiers, names.cabinets
            );
        } else {
            println!("Pedal returned status but no decodable 0x41 name table.");
        }

        let (presets, names) = super::read_device_library().unwrap();
        assert!(presets.len() >= 200);
        println!(
            "Production sync returned {} presets; custom names: {}",
            presets.len(),
            names.is_some()
        );
    }

    #[test]
    #[ignore = "read-only USB capture; requires connected GE150 Pro with MOOER Studio closed"]
    fn captures_read_only_hid_reports_for_tuner_button() {
        let result = run_on_hid_thread(|session| -> Result<(), String> {
            if session.is_none() {
                *session = Some(super::HidSession::open()?);
            }
            let device = &session.as_ref().unwrap().device;
            println!("LISTENING for 30 seconds; hold pedal buttons A+B now.");
            let started = std::time::Instant::now();
            let mut stream = Vec::new();
            let mut counts = BTreeMap::<u8, usize>::new();
            let mut reads = 0usize;
            while started.elapsed() < std::time::Duration::from_secs(30) {
                let mut report = [0u8; 64];
                let count = device
                    .read_timeout(&mut report, 250)
                    .map_err(|error| error.to_string())?;
                if count == 0 {
                    continue;
                }
                reads += 1;
                let payload = if count == 64 && report[0] <= super::REPORT_PAYLOAD_MAX as u8 {
                    &report[1..1 + usize::from(report[0])]
                } else {
                    &report[..count]
                };
                if reads <= 20 {
                    println!(
                        "HID report {reads}: {:02x?}",
                        &payload[..payload.len().min(24)]
                    );
                }
                stream.extend_from_slice(payload);
                for frame in parse_frames(&mut stream) {
                    if frame.len() >= 6 {
                        *counts.entry(frame[4]).or_default() += 1;
                        if frame[4] != command::PRESET_RECORD {
                            println!(
                                "frame 0x{:02X}: {:02x?}",
                                frame[4],
                                &frame[5..frame.len() - 2]
                            );
                        }
                    }
                }
            }
            println!("Read {reads} reports; framed command counts: {counts:?}");
            Ok(())
        });
        result.unwrap();
    }
}
