use ge150_tone::{
    device::{DevicePreset, PedalInterface, UsbPedal},
    EffectModelInfo,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};
use tauri::{AppHandle, Manager};
use tauri_plugin_fs::FsExt;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Bootstrap {
    catalog_path: String,
    starter_preset: Value,
    presets: Vec<String>,
    models: Vec<Vec<EffectModelInfo>>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresetBackup {
    slot: u8,
    name: String,
    raw_data: Vec<u8>,
}

fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn asset_path(app: &AppHandle, relative: &str) -> PathBuf {
    if let Ok(resource_dir) = app.path().resource_dir() {
        let bundled = resource_dir.join(relative);
        if bundled.exists() {
            return bundled;
        }
    }
    project_root().join(relative)
}

fn log_pedal_save(path: &Path, message: &str) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or_default();
        let _ = std::io::Write::write_fmt(&mut file, format_args!("{timestamp} {message}\n"));
    }
}

#[tauri::command]
fn get_bootstrap(app: AppHandle) -> Result<Bootstrap, String> {
    let catalog = asset_path(&app, "catalog/effects_en.xml");
    let starter_preset = ge150_tone::blank_preset_from_catalog(&catalog)?;
    let mut presets = Vec::new();
    if let Ok(entries) = fs::read_dir(project_root().join("generated")) {
        presets.extend(
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("mo"))
                .map(|path| path.to_string_lossy().into_owned()),
        );
    }
    let models = ge150_tone::read_model_catalog(&catalog).map_err(|error| error.to_string())?;
    Ok(Bootstrap {
        catalog_path: catalog.to_string_lossy().into_owned(),
        starter_preset,
        presets,
        models,
    })
}

fn provider_key_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new("com.ge150.tonestudio", "provider-api-key")
        .map_err(|error| error.to_string())
}

#[tauri::command]
fn load_api_key() -> Result<Option<String>, String> {
    match provider_key_entry()?.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
fn save_api_key(api_key: String) -> Result<(), String> {
    let entry = provider_key_entry()?;
    if api_key.is_empty() {
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    } else {
        entry.set_password(&api_key).map_err(|error| error.to_string())
    }
}

#[tauri::command]
fn load_preset(path: String) -> Result<Value, String> {
    let text = fs::read_to_string(path).map_err(|error| error.to_string())?;
    let preset: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    ge150_tone::validate_template(&preset).map_err(|error| error.to_string())?;
    Ok(preset)
}

#[tauri::command]
async fn sync_device_presets() -> Result<Vec<DevicePreset>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        UsbPedal.read_presets()
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn use_device_preset(
    template: Value,
    device_preset: DevicePreset,
    catalog_path: String,
) -> Result<(Value, usize, usize), String> {
    ge150_tone::device_preset_to_json(template, &device_preset, &PathBuf::from(catalog_path))
}

#[tauri::command]
async fn activate_device_preset(slot: u8) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || UsbPedal.activate(slot))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn sync_device_data() -> Result<(Vec<DevicePreset>, Option<ge150_tone::device::CustomModelNames>), String> {
    tauri::async_runtime::spawn_blocking(ge150_tone::device::read_device_library)
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn convert_nam_to_gnr(
    app: AppHandle,
    converter_path: String,
    input_path: String,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let converter = PathBuf::from(converter_path);
        let input = PathBuf::from(input_path);
        if !converter.is_file() {
            return Err("selected NAM converter is not a file".into());
        }
        if input
            .extension()
            .and_then(|ext| ext.to_str())
            .is_none_or(|ext| !ext.eq_ignore_ascii_case("nam"))
        {
            return Err("choose a .nam file".into());
        }
        let input = input.canonicalize().map_err(|error| error.to_string())?;
        if fs::metadata(&input).map_err(|error| error.to_string())?.len() > 512 * 1024 * 1024 {
            return Err("NAM file exceeds the 512 MiB conversion limit".into());
        }
        let cache = app
            .path()
            .app_cache_dir()
            .map_err(|error| error.to_string())?
            .join("converted-amps");
        fs::create_dir_all(&cache).map_err(|error| error.to_string())?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let output = cache.join(format!("nam-conversion-{timestamp}.gnr"));
        let result = Command::new(&converter)
            .arg("--input")
            .arg(&input)
            .arg("--output")
            .arg(&output)
            .output()
            .map_err(|error| format!("could not launch selected converter: {error}"))?;
        if !result.status.success() {
            let message = String::from_utf8_lossy(&result.stderr);
            let _ = fs::remove_file(&output);
            return Err(format!("NAM conversion failed: {}", message.trim()));
        }
        if fs::metadata(&output).map_err(|error| error.to_string())?.len() > 64 * 1024 {
            let _ = fs::remove_file(&output);
            return Err("converter output exceeds the supported GNR size".into());
        }
        let converted = fs::read(&output).map_err(|error| error.to_string())?;
        ge150_tone::device::validate_gnr(&converted)
            .map_err(|error| format!("converter output is invalid: {error}"))?;
        Ok(output.to_string_lossy().into_owned())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn import_custom_amp_gnr(slot: u8, path: String, name: String) -> Result<(), String> {
    if slot >= 20 {
        return Err("custom AMP slot must be between 0 and 19".into());
    }
    let path = PathBuf::from(path);
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_none_or(|ext| !ext.eq_ignore_ascii_case("gnr"))
    {
        return Err("choose a .gnr file".into());
    }
    let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
    if metadata.len() > 64 * 1024 {
        return Err("GNR file exceeds the supported size".into());
    }
    let data = fs::read(path).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        ge150_tone::device::upload_custom_amp_gnr(slot, data, name)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn import_custom_ir_wav(slot: u8, path: String, name: String) -> Result<(), String> {
    if slot >= 20 {
        return Err("custom CAB slot must be between 0 and 19".into());
    }
    let path = PathBuf::from(path);
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_none_or(|ext| !ext.eq_ignore_ascii_case("wav"))
    {
        return Err("choose a .wav file".into());
    }
    let metadata = fs::metadata(&path).map_err(|error| error.to_string())?;
    if metadata.len() > 100 * 1024 * 1024 {
        return Err("WAV file exceeds the 100 MiB import limit".into());
    }
    let data = fs::read(path).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        ge150_tone::device::upload_custom_ir_wav(slot, data, name)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn sync_live_module(
    slot: usize,
    module: Value,
    base_module: ge150_tone::device::RawModule,
    catalog_path: String,
) -> Result<(), String> {
    let payload =
        ge150_tone::encode_live_module(slot, &module, &base_module, &PathBuf::from(catalog_path))?;
    tauri::async_runtime::spawn_blocking(move || UsbPedal.update_module(slot, &payload))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn preview_preset_on_device(
    preset: Value,
    slot: u8,
    base_modules: Vec<Option<ge150_tone::device::RawModule>>,
    catalog_path: String,
) -> Result<(), String> {
    if !(1..=200).contains(&slot) {
        return Err("Choose a GE150 slot from 01A to 50D".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        UsbPedal.activate(slot)?;
        for module_slot in 0..9 {
            let module = &preset["effectModule"][module_slot.to_string()];
            let base = base_modules
                .get(module_slot)
                .and_then(Option::as_ref)
                .ok_or_else(|| format!("missing device baseline for module {module_slot}"))?;
            let data = ge150_tone::encode_live_module(
                module_slot,
                module,
                base,
                &PathBuf::from(&catalog_path),
            )?;
            UsbPedal.update_module(module_slot, &data)?;
        }
        Ok::<_, String>(())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn generate_tone(
    prompt: String,
    preset: Value,
    catalog_path: String,
    base_url: String,
    model: String,
    api_key: String,
) -> Result<(Value, Option<String>), String> {
    tauri::async_runtime::spawn_blocking(move || {
        ge150_tone::generate_tone_from_preset(
            &prompt,
            preset,
            &PathBuf::from(catalog_path),
            &base_url,
            &model,
            (!api_key.is_empty()).then_some(api_key.as_str()),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn export_preset(path: String, preset: Value) -> Result<(), String> {
    ge150_tone::write_new_preset_file(&PathBuf::from(path), &preset)
}

#[tauri::command]
async fn save_preset_to_device(
    app: AppHandle,
    slot: u8,
    preset: Value,
    catalog_path: String,
) -> Result<String, String> {
    if !(1..=200).contains(&slot) {
        return Err("Choose a GE150 slot from 01A to 50D".into());
    }
    let catalog_path = PathBuf::from(catalog_path);
    let backup_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("backups");
    let diagnostic_path = backup_dir
        .parent()
        .unwrap_or(&backup_dir)
        .join("diagnostics/pedal-save.log");
    tauri::async_runtime::spawn_blocking(move || {
        log_pedal_save(&diagnostic_path, &format!("slot={slot} read-backup begin"));
        let library = match UsbPedal.read_presets() {
            Ok(library) => {
                log_pedal_save(&diagnostic_path, &format!("slot={slot} read-backup ok count={}", library.len()));
                library
            }
            Err(error) => {
                log_pedal_save(&diagnostic_path, &format!("slot={slot} read-backup failed: {error}"));
                return Err(error);
            }
        };
        let matches = library.iter().filter(|item| item.slot == slot).collect::<Vec<_>>();
        if matches.len() != 1 {
            return Err(format!("Expected one backup for slot {slot}, found {}", matches.len()));
        }
        let original = matches[0];
        let new_data = ge150_tone::encode_device_preset(&preset, original, &catalog_path)?;
        log_pedal_save(&diagnostic_path, &format!("slot={slot} encoded preset"));

        fs::create_dir_all(&backup_dir).map_err(|error| error.to_string())?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| error.to_string())?
            .as_nanos();
        let backup_path = backup_dir.join(format!("slot-{slot}-{timestamp}.json"));
        let backup = PresetBackup {
            slot,
            name: original.name.clone(),
            raw_data: original.raw_data.clone(),
        };
        let backup_bytes = serde_json::to_vec_pretty(&backup).map_err(|error| error.to_string())?;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&backup_path)
            .and_then(|mut file| std::io::Write::write_all(&mut file, &backup_bytes))
            .map_err(|error| format!("Could not create slot backup: {error}"))?;
        log_pedal_save(&diagnostic_path, &format!("slot={slot} backup created path={}", backup_path.display()));

        log_pedal_save(&diagnostic_path, &format!("slot={slot} usb-write begin bytes={}", new_data.len()));
        let write_result = UsbPedal.write_preset(slot, &new_data);
        log_pedal_save(&diagnostic_path, &format!("slot={slot} usb-write result={:?}", write_result.as_ref().err()));
        log_pedal_save(&diagnostic_path, &format!("slot={slot} verify-read begin"));
        let verification = UsbPedal.read_presets()
            .and_then(|presets| {
                presets
                    .into_iter()
                    .find(|item| item.slot == slot)
                    .ok_or_else(|| format!("Slot {slot} disappeared after write"))
            });
        log_pedal_save(&diagnostic_path, &format!("slot={slot} verify-read result={:?}", verification.as_ref().map(|item| item.raw_data == new_data)));
        if write_result.is_ok()
            && verification.as_ref().is_ok_and(|saved| saved.raw_data == new_data)
        {
            return Ok(backup_path.to_string_lossy().into_owned());
        }

        let failure = write_result.err().unwrap_or_else(|| {
            verification
                .err()
                .unwrap_or_else(|| "device read-back did not match the requested preset".into())
        });
        log_pedal_save(&diagnostic_path, &format!("slot={slot} rollback-write begin"));
        let restored = UsbPedal.write_preset(slot, &original.raw_data).is_ok()
            && UsbPedal.read_presets()
                .ok()
                .and_then(|presets| presets.into_iter().find(|item| item.slot == slot))
                .is_some_and(|item| item.raw_data == original.raw_data);
        log_pedal_save(&diagnostic_path, &format!("slot={slot} rollback result={restored}"));
        if restored {
            Err(format!("Write failed; original slot restored. Backup: {}. Cause: {failure}", backup_path.display()))
        } else {
            Err(format!("Write failed and automatic restore could not be verified. Preserve this backup: {}. Cause: {failure}", backup_path.display()))
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn restore_device_backup(path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let backup: PresetBackup =
            serde_json::from_slice(&fs::read(&path).map_err(|error| error.to_string())?)
                .map_err(|error| error.to_string())?;
        if !(1..=200).contains(&backup.slot) || backup.raw_data.len() != 244 {
            return Err("Backup file is not a valid GE150 raw preset backup".into());
        }
        UsbPedal.write_preset(backup.slot, &backup.raw_data)?;
        let restored = UsbPedal.read_presets()?
            .into_iter()
            .find(|preset| preset.slot == backup.slot)
            .is_some_and(|preset| preset.raw_data == backup.raw_data);
        if restored {
            Ok(())
        } else {
            Err(format!("Could not verify restore for slot {}", backup.slot))
        }
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
fn allow_recording_output(app: AppHandle, path: String) -> Result<(), String> {
    let output = PathBuf::from(path);
    if !output.is_absolute()
        || !output
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"))
    {
        return Err("Recording output must be an absolute WAV file path".into());
    }
    app.fs_scope()
        .allow_file(output)
        .map_err(|error| error.to_string())
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            get_bootstrap,
            load_api_key,
            save_api_key,
            load_preset,
            sync_device_presets,
            sync_device_data,
            convert_nam_to_gnr,
            import_custom_amp_gnr,
            import_custom_ir_wav,
            use_device_preset,
            activate_device_preset,
            sync_live_module,
            preview_preset_on_device,
            generate_tone,
            export_preset,
            save_preset_to_device,
            restore_device_backup,
            allow_recording_output
        ])
        .run(tauri::generate_context!())
        .expect("error while running GE150 Tone Studio");
}
