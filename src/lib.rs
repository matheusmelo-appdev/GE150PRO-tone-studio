use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::{
    collections::HashMap,
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

pub mod device;
pub mod device_commands;

const SLOTS: [&str; 9] = [
    "FX/COMP", "DS/OD", "AMP", "CAB", "NS GATE", "EQ", "MOD", "DELAY", "REVERB",
];
const MODEL_EQUIVALENTS: &str = include_str!("../catalog/model_equivalents.json");

#[derive(Clone, Debug, serde::Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectModelInfo {
    pub name: String,
    pub parameters: Vec<ParameterInfo>,
    pub available: bool,
    pub custom_ir: bool,
    pub custom_amp: bool,
    pub based_on: Option<String>,
}

#[derive(Clone, Debug, serde::Deserialize, Serialize)]
pub struct ParameterInfo {
    pub name: String,
    pub minimum: f64,
    pub maximum: f64,
    pub step: f64,
    pub default: f64,
    pub choices: Vec<String>,
}

struct Args {
    prompt: String,
    template: Option<PathBuf>,
    catalog: PathBuf,
    output: PathBuf,
    model: String,
    base_url: String,
    api_key: Option<String>,
}

pub fn cli_main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let args = parse_args()?;
    let mut preset = if let Some(template) = &args.template {
        let template_text = fs::read_to_string(template)?;
        serde_json::from_str(&template_text).map_err(|error| {
            format!(
                "{} is not a valid GE150 preset JSON file: {error}",
                template.display()
            )
        })?
    } else {
        blank_preset_from_catalog(&args.catalog)
            .map_err(|error| format!("could not build a blank preset: {error}"))?
    };
    validate_template(&preset)?;

    let model_catalog = read_model_catalog(&args.catalog)?;
    let prompt = build_model_prompt(&args.prompt, &preset, &model_catalog)?;
    let generated = ask_model(
        &args.base_url,
        &args.model,
        args.api_key.as_deref(),
        &prompt,
    )?;
    apply_generated_tone(&mut preset, &generated, &model_catalog)?;

    if let Some(parent) = args.output.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    if args.output.extension().and_then(|value| value.to_str()) != Some("mo") {
        return Err("output filename must end in .mo".into());
    }
    if args.output.exists() {
        return Err(format!(
            "{} already exists; choose a new output path",
            args.output.display()
        )
        .into());
    }
    fs::write(&args.output, serde_json::to_string_pretty(&preset)? + "\n")?;

    println!("Wrote GE150 preset: {}", args.output.display());
    if let Some(summary) = generated.get("summary").and_then(Value::as_str) {
        println!("Tone plan: {summary}");
    }
    println!("Effect models and settings were generated for the GE150 catalog.");
    Ok(())
}

fn parse_args() -> Result<Args, Box<dyn Error>> {
    let mut prompt = None;
    let mut template = None;
    let mut catalog = PathBuf::from("catalog/effects_en.xml");
    let mut output = None;
    let mut model = env::var("AI_MODEL")
        .or_else(|_| env::var("OLLAMA_MODEL"))
        .unwrap_or_else(|_| "qwen2.5:7b".to_owned());
    let mut base_url = env::var("OPENAI_BASE_URL")
        .or_else(|_| env::var("OLLAMA_URL"))
        .unwrap_or_else(|_| "http://127.0.0.1:11434/v1".to_owned());
    let mut api_key = env::var("OPENAI_API_KEY").ok();

    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = |args: &mut std::iter::Skip<std::env::Args>,
                     flag: &str|
         -> Result<String, Box<dyn Error>> {
            args.next()
                .ok_or_else(|| format!("{flag} needs a value").into())
        };
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                std::process::exit(0);
            }
            "-p" | "--prompt" => prompt = Some(value(&mut args, &arg)?),
            "-t" | "--template" => template = Some(PathBuf::from(value(&mut args, &arg)?)),
            "-c" | "--catalog" => catalog = PathBuf::from(value(&mut args, &arg)?),
            "-o" | "--output" => output = Some(PathBuf::from(value(&mut args, &arg)?)),
            "-m" | "--model" => model = value(&mut args, &arg)?,
            "--base-url" | "--ollama-url" => base_url = value(&mut args, &arg)?,
            "--api-key" => api_key = Some(value(&mut args, &arg)?),
            other => {
                return Err(
                    format!("unknown argument: {other}\n\nRun with --help for usage.").into(),
                )
            }
        }
    }

    let prompt = prompt.ok_or("missing --prompt")?;
    let output = output
        .unwrap_or_else(|| PathBuf::from("generated").join(format!("{}.mo", file_slug(&prompt))));
    Ok(Args {
        prompt,
        template,
        catalog,
        output,
        model,
        base_url,
        api_key,
    })
}

fn print_help() {
    println!(
        "GE150 AI tone generator\n\n\
Usage:\n  cargo run -- --prompt \"tone\" [options]\n\n\
Options:\n\
  -p, --prompt TEXT       Song, artist, or sound description (required)\n\
  -t, --template PATH     Optional .mo starting preset (default: build from GE150 catalog)\n\
  -c, --catalog PATH      GE150 effect catalog (default: catalog/effects_en.xml)\n\
  -o, --output PATH       Output .mo file (default: generated/<prompt>.mo)\n\
  -m, --model NAME        Model name (default: qwen2.5:7b)\n\
      --base-url URL      OpenAI-compatible API base URL (default: Ollama at http://127.0.0.1:11434/v1)\n\
      --api-key KEY       API key (or set OPENAI_API_KEY)\n\
  -h, --help              Show this help\n\n\
Without --template, AI selects all nine block models and settings from the supported GE150 catalog.\n\
"
    );
}

pub fn validate_template(preset: &Value) -> Result<(), Box<dyn Error>> {
    if preset
        .get("fileInfo")
        .and_then(|v| v.get("schema"))
        .and_then(Value::as_str)
        != Some("GE150PRO Preset")
    {
        return Err("template must have fileInfo.schema = GE150PRO Preset".into());
    }
    let modules = preset
        .get("effectModule")
        .and_then(Value::as_object)
        .ok_or("template is missing effectModule")?;
    for slot in 0..SLOTS.len() {
        let module = modules
            .get(&slot.to_string())
            .ok_or_else(|| format!("template is missing effectModule slot {slot}"))?;
        if !module.get("TYPE").is_some_and(Value::is_number)
            || !module.get("SWITCH").is_some_and(Value::is_number)
            || !module.get("Data").is_some_and(Value::is_object)
        {
            return Err(format!("effectModule slot {slot} has an unexpected structure").into());
        }
    }
    Ok(())
}

pub fn read_model_names(path: &Path) -> Result<Vec<Vec<String>>, Box<dyn Error>> {
    let xml = fs::read_to_string(path)?;
    let doc = roxmltree::Document::parse(&xml)?;
    let names = doc
        .descendants()
        .filter(|node| node.has_tag_name("chain"))
        .map(|chain| {
            chain
                .children()
                .filter(|node| node.is_element())
                .map(|node| node.attribute("name").unwrap_or("unnamed model").to_owned())
                .collect()
        })
        .collect();
    Ok(names)
}

pub fn read_model_parameter_names(path: &Path) -> Result<Vec<Vec<Vec<String>>>, Box<dyn Error>> {
    Ok(read_model_parameter_specs(path)?
        .into_iter()
        .map(|slot| {
            slot.into_iter()
                .map(|model| model.into_iter().map(|parameter| parameter.name).collect())
                .collect()
        })
        .collect())
}

pub fn read_model_parameter_specs(
    path: &Path,
) -> Result<Vec<Vec<Vec<ParameterInfo>>>, Box<dyn Error>> {
    let xml = fs::read_to_string(path)?;
    let doc = roxmltree::Document::parse(&xml)?;
    Ok(doc
        .descendants()
        .filter(|node| node.has_tag_name("chain"))
        .map(|chain| {
            let models: Vec<_> = chain.children().filter(|node| node.is_element()).collect();
            let mut schemas = std::collections::HashMap::new();
            for model in &models {
                let parameters: Vec<ParameterInfo> = model
                    .children()
                    .filter(|node| node.has_tag_name("para"))
                    .filter_map(parse_parameter)
                    .collect();
                if !parameters.is_empty() {
                    if let Some(position) = model.attribute("paraPos") {
                        schemas.entry(position.to_owned()).or_insert(parameters);
                    }
                }
            }
            models
                .iter()
                .map(|model| {
                    let direct: Vec<ParameterInfo> = model
                        .children()
                        .filter(|node| node.has_tag_name("para"))
                        .filter_map(parse_parameter)
                        .collect();
                    if !direct.is_empty() {
                        direct
                    } else {
                        model
                            .attribute("paraPos")
                            .and_then(|position| schemas.get(position))
                            .cloned()
                            .unwrap_or_default()
                    }
                })
                .collect()
        })
        .collect())
}

fn parse_parameter(node: roxmltree::Node<'_, '_>) -> Option<ParameterInfo> {
    let name = node.attribute("name")?.to_owned();
    let value = node.text().unwrap_or("").trim();
    let (mut minimum, mut maximum, step, mut default, choices) =
        if node.attribute("type") == Some("1") {
            let choices: Vec<String> = value
                .split('_')
                .filter(|choice| !choice.is_empty())
                .map(str::to_owned)
                .collect();
            (
                0.0,
                choices.len().saturating_sub(1) as f64,
                1.0,
                0.0,
                choices,
            )
        } else {
            let mut values = value.split('_');
            let min = values.next()?.parse::<f64>().ok()?;
            let max = values.next()?.parse::<f64>().ok()?;
            let step = values.next()?.parse::<f64>().ok()?;
            let default = values
                .next()
                .and_then(|part| part.parse::<f64>().ok())
                .unwrap_or(min);
            (min, max, step, default, Vec::new())
        };
    if matches!(name.as_str(), "GAIN1" | "GAIN2" | "GAIN3") || is_eq_band_gain(&name) {
        minimum = 0.0;
        maximum = 32.0;
        default = 16.0;
    }
    Some(ParameterInfo {
        name,
        minimum,
        maximum,
        step,
        default,
        choices,
    })
}

fn is_eq_band_gain(name: &str) -> bool {
    matches!(
        name,
        "80Hz"
            | "100Hz"
            | "200Hz"
            | "240Hz"
            | "250Hz"
            | "400Hz"
            | "630Hz"
            | "750Hz"
            | "800Hz"
            | "1.6KHz"
            | "2.2KHz"
            | "3.2KHz"
            | "4KHz"
            | "6.6KHz"
    )
}

pub fn read_model_catalog(path: &Path) -> Result<Vec<Vec<EffectModelInfo>>, Box<dyn Error>> {
    let names = read_model_names(path)?;
    let parameters = read_model_parameter_specs(path)?;
    let equivalents: HashMap<String, HashMap<String, String>> =
        serde_json::from_str(MODEL_EQUIVALENTS)?;
    let xml = fs::read_to_string(path)?;
    let doc = roxmltree::Document::parse(&xml)?;
    let metadata: Vec<Vec<(bool, bool, bool, Option<String>)>> = doc
        .descendants()
        .filter(|node| node.has_tag_name("chain"))
        .map(|chain| {
            chain
                .children()
                .filter(|node| node.is_element())
                .map(|node| {
                    let block = chain.attribute("name").unwrap_or("");
                    let name = node.attribute("name").unwrap_or("");
                    let custom_ir = slot_is_cab(block) && node.attribute("import") == Some("1");
                    let custom_amp = slot_is_amp(block) && node.attribute("import") == Some("1");
                    let available =
                        node.attribute("import") != Some("1") && !name.contains("EMPTY");
                    let based_on = equivalents
                        .get(block)
                        .and_then(|models| models.get(name))
                        .cloned();
                    (available, custom_ir, custom_amp, based_on)
                })
                .collect()
        })
        .collect();
    Ok(names
        .into_iter()
        .enumerate()
        .map(|(slot, models)| {
            models
                .into_iter()
                .enumerate()
                .map(|(index, name)| EffectModelInfo {
                    name,
                    parameters: parameters
                        .get(slot)
                        .and_then(|models| models.get(index))
                        .cloned()
                        .unwrap_or_default(),
                    available: metadata
                        .get(slot)
                        .and_then(|models| models.get(index))
                        .map(|info| info.0)
                        .unwrap_or(false),
                    custom_ir: metadata
                        .get(slot)
                        .and_then(|models| models.get(index))
                        .map(|info| info.1)
                        .unwrap_or(false),
                    custom_amp: metadata
                        .get(slot)
                        .and_then(|models| models.get(index))
                        .map(|info| info.2)
                        .unwrap_or(false),
                    based_on: metadata
                        .get(slot)
                        .and_then(|models| models.get(index))
                        .and_then(|info| info.3.clone()),
                })
                .collect()
        })
        .collect())
}

fn slot_is_cab(name: &str) -> bool {
    name == "CAB"
}

fn slot_is_amp(name: &str) -> bool {
    name == "AMP"
}

pub fn blank_preset_from_catalog(path: &Path) -> Result<Value, String> {
    let catalog = read_model_catalog(path).map_err(|error| error.to_string())?;
    let mut modules = Map::new();
    for slot in 0..SLOTS.len() {
        let model = catalog
            .get(slot)
            .and_then(|models| models.first())
            .filter(|model| !model.parameters.is_empty())
            .ok_or_else(|| format!("GE150 catalog has no starter schema for {}", SLOTS[slot]))?;
        let data = model
            .parameters
            .iter()
            .map(|parameter| (parameter.name.clone(), json!(parameter.default as i64)))
            .collect::<Map<_, _>>();
        modules.insert(
            slot.to_string(),
            json!({"TYPE": 0, "SWITCH": 0, "Data": data}),
        );
    }
    Ok(json!({
        "Exp": {
            "FUN_SWITCH": 0, "MODULE_CTRL": 1, "PARA_CTRL": 10,
            "VOL_MAX": 100, "VOL_MIN": 0, "VOL_SWITCH": 1
        },
        "effectModule": modules,
        "fileInfo": {
            "app": "GE150PRO Edit", "app_version": "V1.0.1",
            "device": "GE150PRO", "device_version": "V1.0.0",
            "preset_name": "AI Tone", "schema": "GE150PRO Preset"
        }
    }))
}

pub fn device_preset_to_json(
    mut template: Value,
    device_preset: &device::DevicePreset,
    catalog_path: &Path,
) -> Result<(Value, usize, usize), String> {
    validate_template(&template).map_err(|error| error.to_string())?;
    let catalog = read_model_catalog(catalog_path).map_err(|error| error.to_string())?;
    let mut mapped = 0;
    let mut skipped = 0;
    for (slot, raw) in device_preset.modules.iter().enumerate() {
        let Some(raw) = raw else {
            skipped += 1;
            continue;
        };
        let model_info = catalog
            .get(slot)
            .and_then(|models| models.get(raw.model_index as usize))
            .filter(|model| model.available || model.custom_ir || model.custom_amp);
        let module = &mut template["effectModule"][slot.to_string()];
        module["TYPE"] = json!(raw.model_index);
        module["SWITCH"] = json!(if raw.enabled { 1 } else { 0 });
        let Some(parameters) = model_info
            .map(|model| &model.parameters)
            .filter(|parameters| {
                !parameters.is_empty() && parameters.len() <= raw.parameters.len()
            })
        else {
            skipped += 1;
            continue;
        };
        let mut data = Map::new();
        for (ordinal, parameter) in parameters.iter().enumerate() {
            let index = device_parameter_index(slot, raw.model_index, &parameter.name, ordinal);
            if let Some(value) = raw.parameters.get(index) {
                data.insert(parameter.name.clone(), json!(value));
            }
        }
        module["Data"] = Value::Object(data);
        mapped += 1;
    }
    template["fileInfo"]["preset_name"] = Value::String(clean_preset_name(&device_preset.name));
    Ok((template, mapped, skipped))
}

pub fn encode_device_preset(
    preset: &Value,
    base: &device::DevicePreset,
    catalog_path: &Path,
) -> Result<Vec<u8>, String> {
    if base.raw_data.len() != 244 {
        return Err("Device preset backup has an unexpected size".into());
    }
    validate_template(preset).map_err(|error| error.to_string())?;
    let catalog = read_model_catalog(catalog_path).map_err(|error| error.to_string())?;
    let mut data = base.raw_data.clone();
    let name = clean_preset_name(
        preset
            .pointer("/fileInfo/preset_name")
            .and_then(Value::as_str)
            .unwrap_or("GE150 Preset"),
    );
    if name.is_empty() {
        return Err("Preset name must include at least one ASCII letter or number".into());
    }
    data[..16].fill(b' ');
    data[..name.len()].copy_from_slice(name.as_bytes());

    for slot in 0..SLOTS.len() {
        let module = &preset["effectModule"][slot.to_string()];
        let model = module["TYPE"]
            .as_u64()
            .ok_or("preset model index is invalid")? as u16;
        let enabled = module["SWITCH"]
            .as_i64()
            .ok_or("preset switch state is invalid")?
            != 0;
        let model_info = catalog
            .get(slot)
            .and_then(|models| models.get(model as usize))
            .filter(|model| {
                (model.available || model.custom_ir || model.custom_amp)
                    && !model.parameters.is_empty()
            })
            .ok_or_else(|| format!("no parameter map for {} model {model}", SLOTS[slot]))?;
        let parameters = module["Data"].as_object().ok_or("preset Data is missing")?;
        if model_info
            .parameters
            .iter()
            .any(|parameter| !parameters.contains_key(&parameter.name))
        {
            return Err(format!(
                "{} model {model} is missing a parameter",
                SLOTS[slot]
            ));
        }
        let offset = 16 + slot * 24;
        let mut raw = [0u16; 10];
        if u16::from_le_bytes([base.raw_data[offset + 2], base.raw_data[offset + 3]]) == model {
            for (index, value) in raw.iter_mut().enumerate() {
                let at = offset + 4 + index * 2;
                *value = u16::from_le_bytes([base.raw_data[at], base.raw_data[at + 1]]);
            }
        }
        let mut used = [false; 10];
        for (ordinal, parameter) in model_info.parameters.iter().enumerate() {
            let index = device_parameter_index(slot, model, &parameter.name, ordinal);
            if index >= raw.len() {
                return Err(format!("parameter map for {} is out of range", SLOTS[slot]));
            }
            let number = parameters[&parameter.name].as_i64().ok_or_else(|| {
                format!(
                    "{} value for {} is not an integer",
                    SLOTS[slot], parameter.name
                )
            })?;
            raw[index] = u16::try_from(number).map_err(|_| {
                format!(
                    "{} value for {} is outside device storage range",
                    parameter.name, SLOTS[slot]
                )
            })?;
            used[index] = true;
        }
        data[offset..offset + 2].copy_from_slice(&(if enabled { 1u16 } else { 0 }).to_le_bytes());
        data[offset + 2..offset + 4].copy_from_slice(&model.to_le_bytes());
        for (index, value) in raw.iter().enumerate() {
            let at = offset + 4 + index * 2;
            if used[index] {
                data[at..at + 2].copy_from_slice(&value.to_le_bytes());
            }
        }
    }
    Ok(data)
}

pub fn encode_live_module(
    slot: usize,
    module: &Value,
    base: &device::RawModule,
    catalog_path: &Path,
) -> Result<Vec<u8>, String> {
    if slot >= SLOTS.len() {
        return Err("module slot is out of range".into());
    }
    let model = module["TYPE"]
        .as_u64()
        .ok_or("module model index is invalid")? as u16;
    let enabled = module["SWITCH"]
        .as_i64()
        .ok_or("module switch is invalid")?
        != 0;
    let catalog = read_model_catalog(catalog_path).map_err(|error| error.to_string())?;
    let model_info = catalog
        .get(slot)
        .and_then(|models| models.get(model as usize))
        .filter(|model| {
            (model.available || model.custom_ir || model.custom_amp) && !model.parameters.is_empty()
        })
        .ok_or_else(|| format!("no parameter map for {} model {model}", SLOTS[slot]))?;
    let parameters = module["Data"].as_object().ok_or("module Data is missing")?;
    let mut values = base.parameters.clone();
    for (ordinal, parameter) in model_info.parameters.iter().enumerate() {
        let index = device_parameter_index(slot, model, &parameter.name, ordinal);
        let value = parameters
            .get(&parameter.name)
            .and_then(Value::as_i64)
            .ok_or_else(|| format!("{} value for {} is invalid", SLOTS[slot], parameter.name))?;
        values[index] = u16::try_from(value).map_err(|_| {
            format!(
                "{} value for {} is outside device storage range",
                parameter.name, SLOTS[slot]
            )
        })?;
    }
    let mut bytes = Vec::with_capacity(24);
    bytes.extend_from_slice(&(if enabled { 1u16 } else { 0 }).to_le_bytes());
    bytes.extend_from_slice(&model.to_le_bytes());
    for value in values.into_iter().take(10) {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    Ok(bytes)
}

pub fn write_new_preset_file(path: &Path, preset: &Value) -> Result<(), String> {
    if path.extension().and_then(|ext| ext.to_str()) != Some("mo") {
        return Err("Preset file must use the .mo extension".into());
    }
    validate_template(preset).map_err(|error| error.to_string())?;
    let content = serde_json::to_vec_pretty(preset).map_err(|error| error.to_string())?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    use std::io::Write;
    if let Err(error) = file.write_all(&[content.as_slice(), b"\n"].concat()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error.to_string());
    }
    Ok(())
}

fn build_model_prompt(
    user_prompt: &str,
    preset: &Value,
    model_catalog: &[Vec<EffectModelInfo>],
) -> Result<String, Box<dyn Error>> {
    let modules = preset["effectModule"]
        .as_object()
        .ok_or("template has no effect modules")?;
    let mut context = Vec::new();
    for slot in 0..SLOTS.len() {
        let module = &modules[&slot.to_string()];
        let available_models: Vec<Value> = model_catalog
            .get(slot)
            .into_iter()
            .flatten()
            .enumerate()
            .filter(|(_, model)| model.available && !model.parameters.is_empty())
            .map(|(index, model)| {
                json!({
                    "model_index": index,
                    "model": model.name,
                    "based_on": model.based_on,
                    "parameters": model.parameters,
                })
            })
            .collect();
        let starting_model_index = module["TYPE"].as_u64().unwrap_or(u64::MAX) as usize;
        let starting_model_based_on = model_catalog
            .get(slot)
            .and_then(|models| models.get(starting_model_index))
            .and_then(|model| model.based_on.as_deref());
        context.push(json!({
            "slot": slot,
            "block": SLOTS[slot],
            "starting_model_index": module["TYPE"],
            "starting_model_based_on": starting_model_based_on,
            "starting_parameters": module["Data"],
            "available_models": available_models,
        }));
    }

    Ok(format!(
        "Design a complete practical guitar tone for the GE150 Pro from the blank schema below.\n\
The requested sound is: {user_prompt}\n\n\
The GE150 has nine fixed blocks in the order shown. Choose the best available model independently for each block, even when replacing the starting model; use model_index exactly as listed and never invent a model or parameter.\n\
Use any `based_on` label as a tonal reference only; it is not a promise of exact hardware behavior or an exact recording match.\n\
For every block return model_index, enabled, and every named parameter. Return JSON only with exactly this shape: {{\"preset_name\": string, \"summary\": string, \"blocks\": {{\"0\": {{\"model_index\": integer, \"enabled\": boolean, \"parameters\": {{parameter_name: number}}}}, ... \"8\": ...}}}}.\n\
Parameter metadata includes valid min/max/step and choices where the device uses a selector. EQ gain values are stored offset: 16 means 0 dB and 0..32 maps to -16..+16 dB. Keep all other values within their catalog limits.\n\
Choose settings with sensible gain staging and a useful amp/cab pairing. The description is a target, not a promise of an exact recording match.\n\n\
GE150 schema and available models:\n{}",
        serde_json::to_string_pretty(&context)?
    ))
}

fn ask_model(
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
    prompt: &str,
) -> Result<Value, Box<dyn Error>> {
    let client = Client::builder()
        .timeout(Duration::from_secs(300))
        .build()?;
    let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let mut request = client.post(url).json(&json!({
            "model": model,
            "stream": false,
            "response_format": {"type": "json_object"},
            "messages": [
                {"role": "system", "content": "You are a guitar tone designer. Return only valid JSON following the requested shape."},
                {"role": "user", "content": prompt}
            ]
        }));
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let response = request
        .send()
        .map_err(|error| format!("could not reach AI provider at {base_url} ({error})"))?;
    let status = response.status();
    let body: Value = response
        .json()
        .map_err(|error| format!("AI provider returned invalid JSON: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "AI provider error ({status}): {}",
            body.get("error").unwrap_or(&body)
        )
        .into());
    }
    let content = body
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .ok_or("AI provider response is missing choices[0].message.content")?;
    serde_json::from_str(content)
        .map_err(|error| format!("model returned malformed tone JSON: {error}").into())
}

pub fn apply_generated_tone(
    preset: &mut Value,
    generated: &Value,
    catalog: &[Vec<EffectModelInfo>],
) -> Result<(), String> {
    let name = generated
        .get("preset_name")
        .and_then(Value::as_str)
        .ok_or("AI response is missing preset_name")?;
    let name = clean_preset_name(name);
    if name.is_empty() {
        return Err("AI response produced an empty preset name".into());
    }
    let proposed = generated
        .get("blocks")
        .and_then(Value::as_object)
        .ok_or("AI response is missing blocks")?;
    for slot in 0..SLOTS.len() {
        let key = slot.to_string();
        let suggestion = proposed
            .get(&key)
            .ok_or_else(|| format!("AI response is missing block {slot}"))?;
        let model_index = suggestion
            .get("model_index")
            .and_then(Value::as_u64)
            .ok_or_else(|| format!("AI response block {slot} is missing model_index"))?
            as usize;
        let model = catalog
            .get(slot)
            .and_then(|models| models.get(model_index))
            .filter(|model| model.available && !model.parameters.is_empty())
            .ok_or_else(|| format!("AI selected an unsupported model for {}", SLOTS[slot]))?;
        let enabled = suggestion
            .get("enabled")
            .and_then(Value::as_bool)
            .ok_or_else(|| format!("AI response block {slot} is missing boolean enabled"))?;
        let proposed_parameters = suggestion
            .get("parameters")
            .and_then(Value::as_object)
            .ok_or_else(|| format!("AI response block {slot} is missing parameters"))?;
        let mut data = Map::new();
        for parameter in &model.parameters {
            let suggested = proposed_parameters
                .get(&parameter.name)
                .and_then(Value::as_f64)
                .ok_or_else(|| {
                    format!("AI response omitted a numeric {} parameter", parameter.name)
                })?;
            if !suggested.is_finite() {
                return Err(format!(
                    "AI response value for {} is not finite",
                    parameter.name
                ));
            }
            let value = quantize_parameter(parameter, suggested);
            let json_value = if value.fract() == 0.0 {
                json!(value as i64)
            } else {
                json!(value)
            };
            data.insert(parameter.name.clone(), json_value);
        }
        let module = &mut preset["effectModule"][&key];
        module["TYPE"] = json!(model_index);
        module["SWITCH"] = json!(if enabled { 1 } else { 0 });
        module["Data"] = Value::Object(data);
    }
    preset["fileInfo"]["preset_name"] = Value::String(name);
    Ok(())
}

fn quantize_parameter(parameter: &ParameterInfo, value: f64) -> f64 {
    let clamped = value.clamp(parameter.minimum, parameter.maximum);
    if parameter.step > 0.0 {
        let steps = ((clamped - parameter.minimum) / parameter.step).round();
        (parameter.minimum + steps * parameter.step).clamp(parameter.minimum, parameter.maximum)
    } else {
        clamped
    }
}

pub fn clean_preset_name(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == ' ' || *ch == '-' || *ch == '_')
        .take(16)
        .collect::<String>()
        .trim()
        .to_owned()
}

pub fn file_slug(prompt: &str) -> String {
    let slug = prompt
        .split_whitespace()
        .take(6)
        .map(|word| {
            word.chars()
                .filter(char::is_ascii_alphanumeric)
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if slug.is_empty() {
        "generated tone".to_owned()
    } else {
        slug
    }
}

pub fn device_parameter_index(slot: usize, model: u16, name: &str, ordinal: usize) -> usize {
    if slot == 5 && model == 3 {
        return match name {
            "GAIN1" => 0,
            "GAIN2" => 2,
            "GAIN3" => 4,
            "FREQ1" => 6,
            "FREQ2" => 7,
            "FREQ3" => 8,
            _ => ordinal,
        };
    }
    ordinal
}

#[cfg(test)]
mod tests {
    use super::{
        apply_generated_tone, blank_preset_from_catalog, device, device_parameter_index,
        device_preset_to_json, encode_device_preset, file_slug, read_model_catalog, SLOTS,
    };
    use serde_json::{json, Map, Value};
    use std::{
        fs,
        path::{Path, PathBuf},
    };

    #[test]
    fn output_names_use_spaces() {
        assert_eq!(file_slug("Eternal Rest A7X"), "eternal rest a7x");
        assert_eq!(file_slug("🎸"), "generated tone");
    }

    #[test]
    fn custom_eq_storage_order_matches_named_parameters() {
        assert_eq!(device_parameter_index(5, 3, "GAIN2", 2), 2);
        assert_eq!(device_parameter_index(5, 3, "FREQ1", 1), 6);
        assert_eq!(device_parameter_index(4, 3, "FREQ1", 1), 1);
    }

    #[test]
    fn parameter_schemas_inherit_para_positions_without_tone_examples() {
        let catalog = read_model_catalog(Path::new("catalog/effects_en.xml")).unwrap();
        assert!(catalog[1][1]
            .parameters
            .iter()
            .any(|parameter| parameter.name == "VOLUME"));
        assert!(catalog[2][7]
            .parameters
            .iter()
            .any(|parameter| parameter.name == "MST"));
        assert!(catalog[3][26].custom_ir);
        assert_eq!(catalog[3][26].parameters[0].name, "TUBE");
        assert!(catalog[2][56].custom_amp);
        assert_eq!(catalog[2][56].parameters.len(), 6);
        assert_eq!(
            catalog[0][0].based_on.as_deref(),
            Some("Dunlop GCB95 Cry Baby")
        );
        assert_eq!(
            catalog[1][1].based_on.as_deref(),
            Some("Ibanez TS808 Tube Screamer")
        );
        assert_eq!(catalog[2][6].based_on.as_deref(), Some("Marshall JCM800"));
        assert_eq!(
            catalog[3][5].based_on.as_deref(),
            Some("Marshall 1960A 4×12 cabinet")
        );
        assert!(!catalog[0][3].parameters.is_empty());
        let blank = blank_preset_from_catalog(Path::new("catalog/effects_en.xml")).unwrap();
        assert_eq!(blank["fileInfo"]["preset_name"], "AI Tone");
        assert_eq!(blank["fileInfo"]["app_version"], "V1.0.1");
        assert_eq!(blank["fileInfo"]["device_version"], "V1.0.0");
        assert_eq!(blank["effectModule"]["2"]["TYPE"], 0);
        let eq_gain = catalog[5][3]
            .parameters
            .iter()
            .find(|parameter| parameter.name == "GAIN1")
            .unwrap();
        assert_eq!(
            (eq_gain.minimum, eq_gain.maximum, eq_gain.default),
            (0.0, 32.0, 16.0)
        );
    }

    #[test]
    fn ai_can_select_models_from_catalog_without_a_tone_example() {
        let path = Path::new("catalog/effects_en.xml");
        let catalog = read_model_catalog(path).unwrap();
        let mut preset = blank_preset_from_catalog(path).unwrap();
        let blocks = (0..SLOTS.len())
            .map(|slot| {
                let model_index = if slot == 0 { 3 } else { 0 };
                let model = &catalog[slot][model_index];
                let parameters = model
                    .parameters
                    .iter()
                    .map(|parameter| (parameter.name.clone(), json!(parameter.default)))
                    .collect::<Map<_, _>>();
                (
                    slot.to_string(),
                    json!({"model_index": model_index, "enabled": slot == 2, "parameters": parameters}),
                )
            })
            .collect::<Map<_, _>>();
        apply_generated_tone(
            &mut preset,
            &json!({"preset_name": "Fresh AI Tone", "blocks": blocks}),
            &catalog,
        )
        .unwrap();
        assert_eq!(preset["fileInfo"]["preset_name"], "Fresh AI Tone");
        assert_eq!(preset["effectModule"]["0"]["TYPE"], 3);
        assert_eq!(preset["effectModule"]["2"]["SWITCH"], 1);
    }

    #[test]
    fn ai_can_replace_a_loaded_custom_ir_with_a_catalog_cab() {
        let path = Path::new("catalog/effects_en.xml");
        let catalog = read_model_catalog(path).unwrap();
        let mut preset = blank_preset_from_catalog(path).unwrap();
        let ir_index = catalog[3].iter().position(|model| model.custom_ir).unwrap();
        preset["effectModule"]["3"] = json!({"TYPE": ir_index, "SWITCH": 1, "Data": {"TUBE": 2}});
        let ir_offset = 16 + 3 * 24;
        let mut raw_data = vec![0; 244];
        raw_data[ir_offset + 2..ir_offset + 4].copy_from_slice(&(ir_index as u16).to_le_bytes());
        raw_data[ir_offset + 6..ir_offset + 8].copy_from_slice(&777u16.to_le_bytes());
        let base = device::DevicePreset {
            slot: 1,
            name: "Base".into(),
            modules: vec![None; 9],
            exp_words: vec![],
            raw_data,
        };
        let loaded_raw = encode_device_preset(&preset, &base, path).unwrap();
        assert_eq!(
            u16::from_le_bytes([loaded_raw[ir_offset + 2], loaded_raw[ir_offset + 3]]) as usize,
            ir_index
        );
        assert_eq!(
            u16::from_le_bytes([loaded_raw[ir_offset + 6], loaded_raw[ir_offset + 7]]),
            777
        );

        let blocks = (0..SLOTS.len())
            .map(|slot| {
                let model_index = catalog[slot]
                    .iter()
                    .position(|model| model.available && !model.parameters.is_empty())
                    .unwrap();
                let parameters = catalog[slot][model_index]
                    .parameters
                    .iter()
                    .map(|parameter| (parameter.name.clone(), json!(parameter.default)))
                    .collect::<Map<_, _>>();
                (
                    slot.to_string(),
                    json!({"model_index": model_index, "enabled": true, "parameters": parameters}),
                )
            })
            .collect::<Map<_, _>>();
        apply_generated_tone(
            &mut preset,
            &json!({"preset_name": "Fresh Cab", "blocks": blocks}),
            &catalog,
        )
        .unwrap();
        let cab_index = catalog[3]
            .iter()
            .position(|model| model.available && !model.parameters.is_empty())
            .unwrap();
        assert_eq!(preset["effectModule"]["3"]["TYPE"], cab_index);
        let raw = encode_device_preset(&preset, &base, path).unwrap();
        assert_eq!(
            u16::from_le_bytes([raw[ir_offset + 2], raw[ir_offset + 3]]) as usize,
            cab_index
        );
    }

    #[test]
    fn imported_custom_amp_maps_parameters_and_is_not_marked_incomplete() {
        let path = Path::new("catalog/effects_en.xml");
        let catalog = read_model_catalog(path).unwrap();
        let amp_index = catalog[2]
            .iter()
            .position(|model| model.custom_amp)
            .unwrap();
        let mut raw_data = vec![0; 244];
        let amp_offset = 16 + 2 * 24;
        raw_data[amp_offset + 2..amp_offset + 4].copy_from_slice(&(amp_index as u16).to_le_bytes());
        let raw = device::DevicePreset {
            slot: 1,
            name: "Custom Amp".into(),
            modules: (0..9)
                .map(|slot| {
                    (slot == 2).then(|| device::RawModule {
                        model_index: amp_index as u16,
                        enabled: true,
                        parameters: vec![70, 55, 60, 65, 65, 12, 0, 0, 0, 0],
                    })
                })
                .collect(),
            exp_words: vec![],
            raw_data: raw_data.clone(),
        };
        let blank = blank_preset_from_catalog(path).unwrap();
        let (decoded, mapped, skipped) = device_preset_to_json(blank, &raw, path).unwrap();
        assert_eq!((mapped, skipped), (1, 8));
        assert_eq!(decoded["effectModule"]["2"]["TYPE"], amp_index);
        assert_eq!(decoded["effectModule"]["2"]["Data"]["GAIN"], 70);
        assert!(encode_device_preset(&decoded, &raw, path).is_ok());
    }

    #[test]
    fn ai_parameter_values_are_clamped_to_model_schema() {
        let path = Path::new("catalog/effects_en.xml");
        let catalog = read_model_catalog(path).unwrap();
        let mut preset = blank_preset_from_catalog(path).unwrap();
        let blocks = (0..SLOTS.len())
            .map(|slot| {
                let model_index = if slot == 5 { 3 } else { 0 };
                let model = &catalog[slot][model_index];
                let parameters = model
                    .parameters
                    .iter()
                    .map(|parameter| {
                        let value = match parameter.name.as_str() {
                            "GAIN1" => 99.0,
                            "FREQ1" => 20_000.0,
                            _ => parameter.default,
                        };
                        (parameter.name.clone(), json!(value))
                    })
                    .collect::<Map<_, _>>();
                (
                    slot.to_string(),
                    json!({"model_index": model_index, "enabled": true, "parameters": parameters}),
                )
            })
            .collect::<Map<_, _>>();
        apply_generated_tone(
            &mut preset,
            &json!({"preset_name": "Bounded", "blocks": blocks}),
            &catalog,
        )
        .unwrap();
        assert_eq!(preset["effectModule"]["5"]["Data"]["GAIN1"], 32);
        assert_eq!(preset["effectModule"]["5"]["Data"]["FREQ1"], 18_000);
    }

    #[test]
    fn device_preset_export_round_trips_the_verified_raw_layout() {
        use device::{DevicePreset, RawModule};
        let models: [u16; 9] = [11, 1, 7, 5, 2, 3, 4, 8, 4];
        let switches = [false, true, true, true, true, true, false, false, false];
        let values: [Vec<u16>; 9] = [
            vec![41, 38, 46],
            vec![85, 62, 10],
            vec![44, 50, 45, 65, 60, 93],
            vec![1, 6, 10, 50],
            vec![100, 100, 44],
            vec![13, 16, 19, 16, 18, 16, 398, 1997, 6771],
            vec![50, 50, 50],
            vec![50, 50, 1040, 1, 1040, 1],
            vec![0, 24, 27, 74],
        ];
        let mut raw_data = vec![0; 244];
        raw_data[..16].copy_from_slice(b"SYN Rhythm      ");
        let mut modules = Vec::new();
        for slot in 0..9 {
            let offset = 16 + slot * 24;
            raw_data[offset..offset + 2]
                .copy_from_slice(&(if switches[slot] { 1u16 } else { 0 }).to_le_bytes());
            raw_data[offset + 2..offset + 4].copy_from_slice(&models[slot].to_le_bytes());
            let mut parameters = vec![0; 10];
            for (index, value) in values[slot].iter().enumerate() {
                parameters[index] = *value;
                let at = offset + 4 + index * 2;
                raw_data[at..at + 2].copy_from_slice(&value.to_le_bytes());
            }
            modules.push(Some(RawModule {
                model_index: models[slot],
                enabled: switches[slot],
                parameters,
            }));
        }
        let device = DevicePreset {
            slot: 3,
            name: "SYN Rhythm".into(),
            modules,
            exp_words: vec![0; 6],
            raw_data: raw_data.clone(),
        };
        let template: Value =
            serde_json::from_str(&fs::read_to_string("samples/SYN Rhythm.mo").unwrap()).unwrap();
        let catalog = PathBuf::from("catalog/effects_en.xml");
        let (preset, mapped, skipped) = device_preset_to_json(template, &device, &catalog).unwrap();
        assert_eq!((mapped, skipped), (9, 0));
        assert_eq!(
            encode_device_preset(&preset, &device, &catalog).unwrap(),
            raw_data
        );
    }

    #[test]
    #[ignore = "requires connected GE150 Pro; this test only reads device presets"]
    fn known_factory_preset_round_trips_without_device_writes() {
        let presets = device::read_all_presets().unwrap();
        let source = presets
            .iter()
            .find(|preset| preset.name == "SYN Rhythm")
            .unwrap();
        let template: Value =
            serde_json::from_str(&fs::read_to_string("samples/SYN Rhythm.mo").unwrap()).unwrap();
        let catalog = PathBuf::from("catalog/effects_en.xml");
        let (preset, mapped, skipped) = device_preset_to_json(template, source, &catalog).unwrap();
        assert_eq!((mapped, skipped), (9, 0));
        assert_eq!(
            encode_device_preset(&preset, source, &catalog).unwrap(),
            source.raw_data
        );
    }
}

pub fn generate_tone(
    prompt: &str,
    template_path: &Path,
    catalog_path: &Path,
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
) -> Result<(Value, Option<String>), String> {
    let text = fs::read_to_string(template_path).map_err(|error| {
        format!(
            "could not read template {}: {error}",
            template_path.display()
        )
    })?;
    let preset: Value = serde_json::from_str(&text)
        .map_err(|error| format!("template is not valid GE150 preset JSON: {error}"))?;
    generate_tone_from_preset(prompt, preset, catalog_path, base_url, model, api_key)
}

pub fn generate_tone_from_preset(
    prompt: &str,
    mut preset: Value,
    catalog_path: &Path,
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
) -> Result<(Value, Option<String>), String> {
    validate_template(&preset).map_err(|error| error.to_string())?;
    let catalog = read_model_catalog(catalog_path).map_err(|error| error.to_string())?;
    let model_prompt =
        build_model_prompt(prompt, &preset, &catalog).map_err(|error| error.to_string())?;
    let generated =
        ask_model(base_url, model, api_key, &model_prompt).map_err(|error| error.to_string())?;
    apply_generated_tone(&mut preset, &generated, &catalog)?;
    Ok((
        preset,
        generated
            .get("summary")
            .and_then(Value::as_str)
            .map(str::to_owned),
    ))
}
