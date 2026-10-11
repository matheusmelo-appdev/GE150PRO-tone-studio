import { useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm, open, save } from "@tauri-apps/plugin-dialog";
import appIcon from "../src-tauri/icons/source.svg?url";
import ampHeadArt from "./assets/guitar-amp-head-front.svg?url";
import cabinetArt from "./assets/guitar-amp-cabinet-2x12.svg?url";
import pedalArt from "./assets/guitar-pedal.svg?url";
import Tuner from "./TunerModal";

type Parameter = number | string;
type Module = { TYPE: number; SWITCH: number; Data: Record<string, Parameter> };
type Preset = {
  Exp: Record<string, number>;
  effectModule: Record<string, Module>;
  fileInfo: Record<string, string>;
};
type DeviceModule = { modelIndex: number; enabled: boolean; parameters: number[] };
type DevicePreset = { slot: number; name: string; modules: (DeviceModule | null)[]; expWords: number[]; rawData: number[] };
type CustomModelNames = { amplifiers: string[]; cabinets: string[] };
type ParameterInfo = { name: string; minimum: number; maximum: number; step: number; default: number; choices: string[] };
type ModelInfo = { name: string; parameters: ParameterInfo[]; available: boolean; customIr: boolean; customAmp: boolean; basedOn: string | null };
type Bootstrap = {
  catalogPath: string;
  starterPreset: Preset;
  presets: string[];
  models: ModelInfo[][];
};

function storedNames(key: string) {
  try {
    const parsed = JSON.parse(localStorage.getItem(key) || "[]");
    return Array.isArray(parsed) ? parsed.filter((name): name is string => typeof name === "string").slice(0, 20) : [];
  } catch {
    return [];
  }
}

function importName(path: string) {
  const filename = path.split(/[\\/]/).pop() || "CUSTOM MODEL";
  const stem = filename.replace(/\.[^.]+$/, "");
  let name = "";
  for (const character of stem) {
    if (new TextEncoder().encode(name + character).length > 16) break;
    name += character;
  }
  return name || "CUSTOM MODEL";
}

const BLOCKS = ["FX / COMP", "DS / OD", "AMP", "CAB", "NS GATE", "EQ", "MOD", "DELAY", "REVERB"];
const DEFAULT_URL = "http://127.0.0.1:11434/v1";

export default function App() {
  const [bootstrap, setBootstrap] = useState<Bootstrap | null>(null);
  const [computerPresets, setComputerPresets] = useState<string[]>([]);
  const [template, setTemplate] = useState<Preset | null>(null);
  const [activeComputerPath, setActiveComputerPath] = useState<string | null>(null);
  const [preset, setPreset] = useState<Preset | null>(null);
  const [devicePresets, setDevicePresets] = useState<DevicePreset[]>([]);
  const [customAmpNames, setCustomAmpNames] = useState<string[]>(() => storedNames("custom-amp-names"));
  const [customCabNames, setCustomCabNames] = useState<string[]>(() => storedNames("custom-cab-names"));
  const [selectedDevice, setSelectedDevice] = useState<number | null>(null);
  const [targetSlot, setTargetSlot] = useState<number | null>(null);
  const [aiLoadOnTarget, setAiLoadOnTarget] = useState(false);
  const [namConverterPath, setNamConverterPath] = useState(localStorage.getItem("nam-converter-path") || "");
  const [importTargetIndex, setImportTargetIndex] = useState<number | null>(null);
  const [customImportTab, setCustomImportTab] = useState<"amp" | "cab">("amp");
  const [tunerOpen, setTunerOpen] = useState(false);
  const [liveLink, setLiveLink] = useState(false);
  const [dirty, setDirty] = useState(false);
  const [lastBackupPath, setLastBackupPath] = useState("");
  const [selectedBlock, setSelectedBlock] = useState(2);
  const [search, setSearch] = useState("");
  const [brief, setBrief] = useState("");
  const [summary, setSummary] = useState("");
  const [baseUrl, setBaseUrl] = useState(localStorage.getItem("provider-url") || DEFAULT_URL);
  const [model, setModel] = useState(localStorage.getItem("provider-model") || "qwen2.5:7b");
  const [apiKey, setApiKey] = useState("");
  const [apiKeySaved, setApiKeySaved] = useState(false);
  const [busy, setBusy] = useState(false);
  const [incomplete, setIncomplete] = useState(false);
  const [startupState, setStartupState] = useState<"loading" | "failed" | "ready">("loading");
  const [startupError, setStartupError] = useState("");
  const [status, setStatus] = useState("Starting Tone Studio…");
  const liveTimers = useRef<Record<number, ReturnType<typeof setTimeout>>>({});

  useEffect(() => {
    localStorage.setItem("provider-url", baseUrl);
  }, [baseUrl]);
  useEffect(() => {
    localStorage.setItem("provider-model", model);
  }, [model]);
  useEffect(() => {
    localStorage.setItem("custom-amp-names", JSON.stringify(customAmpNames));
  }, [customAmpNames]);
  useEffect(() => {
    localStorage.setItem("custom-cab-names", JSON.stringify(customCabNames));
  }, [customCabNames]);
  useEffect(() => {
    localStorage.setItem("nam-converter-path", namConverterPath);
  }, [namConverterPath]);

  useEffect(() => {
    void initializeApp();
  }, []);

  const filteredDevicePresets = useMemo(() => {
    const query = search.trim().toLowerCase();
    return devicePresets.filter((item) => !query || item.name.toLowerCase().includes(query) || String(item.slot).includes(query));
  }, [devicePresets, search]);

  async function syncDevice(config = bootstrap): Promise<string | null> {
    setBusy(true);
    setStatus("Reading device preset library…");
    try {
      const [items, names] = await invoke<[DevicePreset[], CustomModelNames | null]>("sync_device_data");
      setDevicePresets(items);
      setTargetSlot((current) => current !== null && items.some((item) => item.slot === current) ? current : null);
      if (names) {
        setCustomAmpNames(names.amplifiers);
        setCustomCabNames(names.cabinets);
        setStatus(`GE150 Pro · ${items.length} presets · ${names.amplifiers.filter(Boolean).length} custom amps · ${names.cabinets.filter(Boolean).length} IRs`);
      } else {
        const localCount = customAmpNames.filter(Boolean).length + customCabNames.filter(Boolean).length;
        setStatus(`GE150 Pro · ${items.length} presets · pedal omitted name table${localCount ? ` · showing ${localCount} remembered import names` : ""}`);
      }
      return null;
    } catch (error) {
      const message = `Device not synced · ${String(error)}${config ? " · close MOOER Studio and retry" : ""}`;
      setStatus(message);
      return message;
    } finally {
      setBusy(false);
    }
  }

  async function initializeApp() {
    setStartupState("loading");
    setStartupError("");
    try {
      let config = bootstrap;
      if (!config) {
        config = await invoke<Bootstrap>("get_bootstrap");
        setBootstrap(config);
        setComputerPresets(config.presets);
        setTemplate(config.starterPreset);
        void invoke<string | null>("load_api_key")
          .then((savedKey) => {
            setApiKey(savedKey ?? "");
            setApiKeySaved(!!savedKey);
          })
          .catch((error) => setStatus(`Could not load saved API key · ${String(error)}`));
      }
      const syncError = await syncDevice(config);
      if (syncError) {
        setStartupError(syncError);
        setStartupState("failed");
        return;
      }
      setStatus("All 200 preset slots loaded · set your LIVE PEDAL default");
      let enableLive = localStorage.getItem("live-pedal-default");
      if (enableLive === null) {
        const accepted = await confirm(
          "Make LIVE PEDAL the default? Selecting a device preset will activate it, and edits/AI previews will update the pedal immediately. Changes are not saved to flash; Save to Pedal remains separate.",
          { title: "Default live mode", kind: "warning", okLabel: "Enable live by default", cancelLabel: "Keep live off" },
        );
        enableLive = accepted ? "true" : "false";
        localStorage.setItem("live-pedal-default", enableLive);
      }
      setLiveLink(enableLive === "true");
      setStartupState("ready");
      if (enableLive === "true") setStatus("All 200 presets loaded · LIVE PEDAL default enabled");
    } catch (error) {
      const message = String(error);
      setStartupError(message);
      setStatus(`Startup failed · ${message}`);
      setStartupState("failed");
    }
  }

  async function loadComputerPreset(path: string) {
    if (dirty && !(await confirm("Discard unsaved changes to the current tone?", { title: "Unsaved preset", kind: "warning" }))) return;
    try {
      const loaded = await invoke<Preset>("load_preset", { path });
      setTemplate(loaded);
      setPreset(loaded);
      setActiveComputerPath(path);
      setSelectedDevice(null);
      setIncomplete(false);
      setDirty(false);
      setSummary("");
      setStatus(`Loaded ${path.split(/[\\/]/).pop()}`);
    } catch (error) {
      setStatus(`Could not load preset · ${String(error)}`);
    }
  }

  async function addComputerPreset() {
    const path = await open({ multiple: false, filters: [{ name: "GE150 preset", extensions: ["mo"] }] });
    if (typeof path !== "string") return;
    setComputerPresets((items) => items.includes(path) ? items : [...items, path]);
    await loadComputerPreset(path);
  }

  async function chooseNamConverter() {
    const path = await open({ multiple: false });
    if (typeof path !== "string") return;
    setNamConverterPath(path);
    setStatus(`NAM converter selected · ${path.split(/[\\/]/).pop()}`);
  }

  function rememberCustomName(isAmp: boolean, slot: number, name: string) {
    const update = (current: string[]) => {
      const next = Array.from({ length: 20 }, (_, index) => current[index] || "");
      next[slot] = name;
      return next;
    };
    if (isAmp) setCustomAmpNames(update);
    else setCustomCabNames(update);
  }

  async function importCustomModel(kind: "nam" | "gnr" | "wav") {
    if (busy || !devicePresets.length) return;
    const isAmp = kind !== "wav";
    const importBlock = isAmp ? 2 : 3;
    const importModule = preset?.effectModule[String(importBlock)];
    const targetModels = (bootstrap?.models[importBlock] || [])
      .map((model, index) => ({ model, index }))
      .filter(({ model }) => isAmp ? model.customAmp : model.customIr);
    const currentModelIndex = importModule && targetModels.some(({ index }) => index === importModule.TYPE) ? importModule.TYPE : null;
    const target = targetModels.find(({ index }) => index === importTargetIndex)
      ?? targetModels.find(({ index }) => index === currentModelIndex)
      ?? targetModels[0];
    if (!target) {
      setStatus(`No custom ${isAmp ? "AMP" : "CAB"} slots are available in the catalog`);
      return;
    }
    let converter = namConverterPath;
    if (kind === "nam" && !converter) {
      const selected = await open({ multiple: false });
      if (typeof selected !== "string") return;
      converter = selected;
      setNamConverterPath(selected);
    }
    const extensions = kind === "nam" ? ["nam"] : kind === "gnr" ? ["gnr"] : ["wav"];
    const path = await open({ multiple: false, filters: [{ name: kind.toUpperCase(), extensions }] });
    if (typeof path !== "string") return;
    const name = importName(path);
    let gnrPath = path;
    if (kind === "nam") {
      setBusy(true);
      setStatus("Converting NAM with the selected local MOOER CLI…");
      try {
        gnrPath = await invoke<string>("convert_nam_to_gnr", { converterPath: converter, inputPath: path });
      } catch (error) {
        setStatus(`NAM conversion failed · ${String(error)}`);
        setBusy(false);
        return;
      }
      setBusy(false);
    }
    const modelLabel = modelDisplayName(target.model, isAmp ? 2 : 3, target.index, customAmpNames, customCabNames, false);
    const accepted = await confirm(
      `Overwrite custom ${isAmp ? "AMP" : "CAB/IR"} slot “${modelLabel}” with “${name}”? The pedal's custom audio slot has no backup/restore path in this app.`,
      { title: `Import custom ${isAmp ? "AMP" : "IR"}`, kind: "warning", okLabel: "Upload to pedal", cancelLabel: "Cancel" },
    );
    if (!accepted) return;
    setBusy(true);
    setStatus(`Uploading ${name} to ${modelLabel}…`);
    try {
      const slot = targetModels.findIndex(({ index }) => index === target.index);
      await invoke(isAmp ? "import_custom_amp_gnr" : "import_custom_ir_wav", {
        slot,
        path: isAmp ? gnrPath : path,
        name,
      });
      rememberCustomName(isAmp, slot, name);
      setStatus(`Uploaded ${name} to ${modelLabel} · import name remembered locally`);
    } catch (error) {
      setStatus(`Custom model upload failed · ${String(error)}`);
    } finally {
      setBusy(false);
    }
  }

  async function chooseDevicePreset(item: DevicePreset, index: number) {
    if (!bootstrap || busy) return;
    if (dirty && !(await confirm("Discard unsaved changes before switching pedal presets?", { title: "Unsaved preset", kind: "warning" }))) return;
    Object.values(liveTimers.current).forEach(clearTimeout);
    liveTimers.current = {};
    setBusy(true);
    try {
      const baseTemplate = template ?? bootstrap.starterPreset;
      setTemplate(baseTemplate);
      const [loaded, mapped, skipped] = await invoke<[Preset, number, number]>("use_device_preset", {
        template: baseTemplate,
        devicePreset: item,
        catalogPath: bootstrap.catalogPath,
      });
      if (liveLink) {
        await invoke("activate_device_preset", { slot: item.slot });
      }
    setPreset(loaded);
      setActiveComputerPath(null);
      setSelectedDevice(index);
      setTargetSlot(item.slot);
      setSelectedBlock(2);
      setIncomplete(skipped > 0);
      setDirty(false);
      setSummary("");
      setStatus(`Loaded ${presetSlotLabel(item.slot)}${liveLink ? " · activated on pedal" : " · pedal unchanged"} · ${mapped} blocks decoded · ${skipped} raw-only`);
    } catch (error) {
      setStatus(`Could not switch to device preset · ${String(error)}`);
    } finally {
      setBusy(false);
    }
  }

  async function generateTone() {
    const startingPreset = preset ?? template;
    if (!startingPreset || !bootstrap || !brief.trim()) return;
    if (aiLoadOnTarget && targetSlot === null) {
      setStatus("Choose a pedal target or turn off LOAD ON TARGET");
      return;
    }
    setBusy(true);
    setStatus(`Designing with ${model}…`);
    try {
      const [generated, plan] = await invoke<[Preset, string | null]>("generate_tone", {
        prompt: brief,
        preset: startingPreset,
        catalogPath: bootstrap.catalogPath,
        baseUrl,
        model,
        apiKey,
      });
      setPreset(generated);
      setActiveComputerPath(null);
      setSummary(plan || "");
      setIncomplete(false);
      setDirty(true);
      if (aiLoadOnTarget) {
        await previewPresetOnTarget(generated, true);
      } else {
        setSelectedDevice(null);
        await exportPresetValue(generated);
      }
    } catch (error) {
      setStatus(`Generation failed · ${String(error)}`);
    } finally {
      setBusy(false);
    }
  }

  async function saveApiKey() {
    try {
      await invoke("save_api_key", { apiKey });
      setApiKeySaved(!!apiKey);
      setStatus(apiKey ? "API key saved in the system credential store" : "Saved API key removed");
    } catch (error) {
      setApiKeySaved(false);
      setStatus(`Could not save API key · ${String(error)}`);
    }
  }

  async function exportPresetValue(value: Preset) {
    const name = value.fileInfo?.preset_name || "GE150 Tone";
    const path = await save({ defaultPath: `${name}.mo`, filters: [{ name: "GE150 preset", extensions: ["mo"] }] });
    if (!path) {
      setStatus("AI tone ready · no .mo file was saved");
      return false;
    }
    try {
      await invoke("export_preset", { path, preset: value });
      setStatus(`Exported ${path.split(/[\\/]/).pop()} · transfer it with MOOER Studio`);
      return true;
    } catch (error) {
      setStatus(`Export failed · ${String(error)}`);
      return false;
    }
  }

  async function exportPreset() {
    if (!preset || incomplete) return;
    await exportPresetValue(preset);
  }

  async function savePresetToDevice(value = preset, allowBusy = false): Promise<boolean> {
    if (!value || targetSlot === null || (incomplete && !allowBusy) || (busy && !allowBusy) || !bootstrap) return false;
    const targetIndex = devicePresets.findIndex((item) => item.slot === targetSlot);
    if (targetIndex < 0) {
      setStatus("The selected target is not in the synced device library");
      return false;
    }
    const target = devicePresets[targetIndex];
    const label = presetSlotLabel(target.slot);
    const accepted = await confirm(
      `Overwrite pedal preset ${label} “${target.name}”? The app will create a raw backup first and verify the saved data.`,
      { title: "Save to GE150 Pro", kind: "warning", okLabel: "Back up and save", cancelLabel: "Cancel" },
    );
    if (!accepted) {
      setStatus("Target load cancelled · preset remains in the editor");
      return false;
    }
    setBusy(true);
    setStatus(`Backing up and saving ${label}…`);
    try {
      const backupPath = await invoke<string>("save_preset_to_device", {
        slot: target.slot,
        preset: value,
        catalogPath: bootstrap.catalogPath,
      });
      setLastBackupPath(backupPath);
      const updated = await invoke<DevicePreset[]>("sync_device_presets");
      setDevicePresets(updated);
      const savedIndex = updated.findIndex((item) => item.slot === target.slot);
      setSelectedDevice(savedIndex);
      setTargetSlot(target.slot);
      setActiveComputerPath(null);
      setPreset(value);
      setDirty(false);
      setStatus(`Saved and verified ${label} · backup kept at ${backupPath}`);
      return true;
    } catch (error) {
      setStatus(`Pedal save failed · ${String(error)}`);
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function previewPresetOnTarget(value = preset, allowBusy = false): Promise<boolean> {
    if (!value || targetSlot === null || (incomplete && !allowBusy) || (busy && !allowBusy) || !bootstrap) return false;
    const targetIndex = devicePresets.findIndex((item) => item.slot === targetSlot);
    if (targetIndex < 0) {
      setStatus("The selected target is not in the synced device library");
      return false;
    }
    const target = devicePresets[targetIndex];
    const label = presetSlotLabel(target.slot);
    const accepted = await confirm(
      `Load this tone to ${label} “${target.name}” now? The app activates the slot and sends its modules, but does not save to pedal flash.`,
      { title: "Load on target", kind: "warning", okLabel: "Load without saving", cancelLabel: "Cancel" },
    );
    if (!accepted) {
      setStatus("Target load cancelled · preset remains in the editor");
      return false;
    }
    setBusy(true);
    setStatus(`Loading tone to ${label} · not saving to flash…`);
    try {
      await invoke("preview_preset_on_device", {
        preset: value,
        slot: target.slot,
        baseModules: target.modules,
        catalogPath: bootstrap.catalogPath,
      });
      setSelectedDevice(targetIndex);
      setTargetSlot(target.slot);
      setPreset(value);
      setIncomplete(false);
      setDirty(true);
      setStatus(`Loaded to ${label} · changes are live but not saved to flash`);
      return true;
    } catch (error) {
      setStatus(`Target load failed; the pedal may have changed partially · ${String(error)}`);
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function activateOnPedal() {
    if (selectedDevice === null || busy) return;
    const item = devicePresets[selectedDevice];
    const accepted = await confirm(
      `Select ${presetSlotLabel(item.slot)} “${item.name}” on the pedal now? This changes the active pedal preset but does not save edits.`,
      { title: "Activate on GE150 Pro", kind: "warning", okLabel: "Activate", cancelLabel: "Cancel" },
    );
    if (!accepted) return;
    try {
      await invoke("activate_device_preset", { slot: item.slot });
      setStatus(`Activated ${presetSlotLabel(item.slot)} on the pedal`);
    } catch (error) {
      setStatus(`Could not activate pedal preset · ${String(error)}`);
    }
  }

  async function setLivePedal(enabled: boolean) {
    if (!enabled) {
      Object.values(liveTimers.current).forEach(clearTimeout);
      liveTimers.current = {};
      setLiveLink(false);
      localStorage.setItem("live-pedal-default", "false");
      setStatus("LIVE PEDAL off · edits stay local");
      return;
    }
    const hasTarget = selectedDevice !== null;
    if (!bootstrap || busy || (hasTarget && !preset)) return;
    const accepted = await confirm(
      hasTarget
        ? "Activate the selected pedal slot and link this tone? If it has unsaved edits, the app will send the current tone to the pedal. Changes are not saved to flash."
        : "Make LIVE PEDAL the default? After you select a device preset, slot changes, knob edits, and AI previews will update the pedal immediately. Changes are not saved to flash.",
      { title: hasTarget ? "Enable live pedal control" : "Default live mode", kind: "warning", okLabel: hasTarget ? "Enable live" : "Enable by default", cancelLabel: "Cancel" },
    );
    if (!accepted) return;
    if (!hasTarget) {
      setLiveLink(true);
      localStorage.setItem("live-pedal-default", "true");
      setStatus("LIVE PEDAL default enabled · select a device preset to activate it");
      return;
    }
    setBusy(true);
    try {
      if (dirty) {
        await invoke("preview_preset_on_device", {
          preset,
          slot: devicePresets[selectedDevice].slot,
          baseModules: devicePresets[selectedDevice].modules,
          catalogPath: bootstrap.catalogPath,
        });
      } else {
        await invoke("activate_device_preset", { slot: devicePresets[selectedDevice].slot });
      }
      setLiveLink(true);
      localStorage.setItem("live-pedal-default", "true");
      setStatus("LIVE PEDAL connected · knob edits and AI tones will preview immediately");
    } catch (error) {
      setLiveLink(false);
      setStatus(`Could not enable LIVE PEDAL · ${String(error)}`);
    } finally {
      setBusy(false);
    }
  }

  async function restoreLastBackup() {
    if (!lastBackupPath || busy) return;
    const slot = selectedDevice === null ? null : devicePresets[selectedDevice]?.slot;
    const accepted = await confirm(
      "Restore the raw backup to its original pedal slot? This replaces that slot with the backup copy.",
      { title: "Restore pedal backup", kind: "warning", okLabel: "Restore", cancelLabel: "Cancel" },
    );
    if (!accepted) return;
    setBusy(true);
    try {
      await invoke("restore_device_backup", { path: lastBackupPath });
      const updated = await invoke<DevicePreset[]>("sync_device_presets");
      setDevicePresets(updated);
      const restoredIndex = slot === null ? -1 : updated.findIndex((item) => item.slot === slot);
      if (restoredIndex >= 0 && template && bootstrap) {
        const restored = await invoke<[Preset, number, number]>("use_device_preset", {
          template,
          devicePreset: updated[restoredIndex],
          catalogPath: bootstrap.catalogPath,
        });
        const restoredPreset = restored[0];
        setPreset(restoredPreset);
        setSelectedDevice(restoredIndex);
        setIncomplete(restored[2] > 0);
      }
      setDirty(false);
      setStatus("Backup restored and verified on the pedal");
    } catch (error) {
      setStatus(`Restore failed · ${String(error)} · backup remains at ${lastBackupPath}`);
    } finally {
      setBusy(false);
    }
  }

  function updateModule(slot: number, update: Partial<Module>) {
    const current = preset?.effectModule[String(slot)];
    if (!current) return;
    const nextModule = { ...current, ...update };
    setPreset((current) => {
      if (!current) return current;
      const key = String(slot);
      return {
        ...current,
        effectModule: {
          ...current.effectModule,
          [key]: nextModule,
        },
      };
    });
    setDirty(true);
    if (liveLink && selectedDevice !== null && bootstrap) {
      const baseModule = devicePresets[selectedDevice]?.modules[slot];
      if (!baseModule) return;
      clearTimeout(liveTimers.current[slot]);
      liveTimers.current[slot] = setTimeout(() => {
        void invoke("sync_live_module", {
          slot,
          module: nextModule,
          baseModule,
          catalogPath: bootstrap.catalogPath,
        }).catch((error) => setStatus(`Live pedal update failed · ${String(error)}`));
      }, 130);
    }
  }

  function updateParameter(slot: number, name: string, value: number) {
    const module = preset?.effectModule[String(slot)];
    if (!module) return;
    updateModule(slot, { Data: { ...module.Data, [name]: value } });
  }

  function changeModel(slot: number, type: number) {
    const module = preset?.effectModule[String(slot)];
    const modelInfo = bootstrap?.models[slot]?.[type];
    if (!module || !modelInfo?.available || !modelInfo.parameters.length) return;
    const data = Object.fromEntries(modelInfo.parameters.map((parameter) => [parameter.name, module.Data[parameter.name] ?? parameter.default]));
    updateModule(slot, { TYPE: type, Data: data });
    setIncomplete(false);
  }

  const module = preset?.effectModule[String(selectedBlock)];
  const models = bootstrap?.models[selectedBlock] || [];
  const modelInfo = module ? models[module.TYPE] : null;
  const importingAmp = customImportTab === "amp";
  const importingCab = !importingAmp;
  const importBlock = importingAmp ? 2 : 3;
  const importModels = bootstrap?.models[importBlock] || [];
  const importModule = preset?.effectModule[String(importBlock)];
  const customTargets = importModels
    .map((model, index) => ({ model, index }))
    .filter(({ model }) => importingAmp ? model.customAmp : model.customIr);
  const currentCustomTarget = importModule && customTargets.some(({ index }) => index === importModule.TYPE) ? importModule.TYPE : null;
  const activeImportTarget = customTargets.find(({ index }) => index === importTargetIndex)
    ?? customTargets.find(({ index }) => index === currentCustomTarget)
    ?? customTargets[0];
  const currentName = preset?.fileInfo?.preset_name || "Choose a preset to get started";
  const currentTarget = targetSlot === null ? null : devicePresets.find((item) => item.slot === targetSlot) ?? null;

  if (startupState !== "ready") {
    return <div className="startup-cover" aria-busy={startupState === "loading"}>
      <section className="startup-card" aria-live="polite">
        <span className={`startup-spinner ${startupState === "failed" ? "failed" : ""}`} aria-hidden="true" />
        <small>GE150 TONE STUDIO · DEVICE SYNC</small>
        <h1>{startupState === "loading" ? "Loading the pedal library" : "Device sync required"}</h1>
        <p>{startupState === "loading" ? status : startupError}</p>
        {startupState === "loading" ? <p className="startup-detail">Reading all 200 preset slots and custom model data before unlocking the editor…</p> : <button className="button button-primary" disabled={busy} onClick={() => void initializeApp()}>{busy ? "SYNCING…" : "RETRY FULL SYNC"}</button>}
      </section>
    </div>;
  }

  return (
    <div className="studio">
      <header className="topbar">
        <div className="brand"><img className="brand-mark" src={appIcon} alt=""/><div><b>GE150 / TONE LAB</b><small>AMP & EFFECTS STUDIO</small></div></div>
        <div className="top-preset"><small>PRESET</small>{preset ? <input value={currentName} onChange={(e) => { setPreset((p) => p ? { ...p, fileInfo: { ...p.fileInfo, preset_name: e.target.value.slice(0, 16) } } : p); setDirty(true); }} /> : <span>No preset loaded</span>}</div>
        <div className="top-actions"><span className={`connection ${devicePresets.length ? "online" : ""}`}><i />{devicePresets.length ? `GE150 · ${devicePresets.length} PRESETS${customAmpNames.length ? ` · ${customAmpNames.filter(Boolean).length} AMP / ${customCabNames.filter(Boolean).length} IR` : ""}` : "DEVICE OFFLINE"}</span><button className="button button-secondary tuner-open" onClick={() => setTunerOpen(true)}>TUNER</button><label className={`live-link ${liveLink ? "enabled" : ""}`}><input type="checkbox" checked={liveLink} disabled={busy || !devicePresets.length} onChange={(e) => void setLivePedal(e.target.checked)} /><span>LIVE PEDAL</span></label>{selectedDevice !== null && <button className="button button-secondary" disabled={busy} onClick={() => void activateOnPedal()}>ACTIVATE</button>}{lastBackupPath && <button className="button button-secondary" disabled={busy} onClick={() => void restoreLastBackup()}>RESTORE BACKUP</button>}<button className="button button-secondary" disabled={!preset || incomplete} onClick={() => void exportPreset()}>EXPORT .MO</button><button className="button button-primary top-save" disabled={!preset || targetSlot === null || incomplete || busy} onClick={() => void (selectedDevice === null ? previewPresetOnTarget() : savePresetToDevice())}>{selectedDevice === null ? "LOAD ON TARGET" : "SAVE TO PEDAL"}</button></div>
      </header>

      <div className="workspace">
        <aside className="library panel">
          <section className="library-section device-library">
            <div className="section-heading"><div><small>HARDWARE</small><h2>Device presets</h2></div><button className="icon-button" aria-label="Sync device presets" title="Sync device" disabled={busy} onClick={() => void syncDevice()}><svg aria-hidden="true" viewBox="0 0 20 20"><path d="M16 7a6.2 6.2 0 0 0-11-2L3 7m0-4v4h4M4 13a6.2 6.2 0 0 0 11 2l2-2m0 4v-4h-4" /></svg></button></div>
            <div className="library-count">{devicePresets.length ? `${devicePresets.length} PRESETS` : "NOT SYNCED"}<span>USB LINK</span></div>
            <input className="search" placeholder="Search device presets" value={search} onChange={(e) => setSearch(e.target.value)} />
            <div className="preset-list device-list">
              {filteredDevicePresets.map((item) => {
                const realIndex = devicePresets.indexOf(item);
                return <button key={item.slot} className={`preset-row ${selectedDevice === realIndex ? "selected" : ""}`} onClick={() => void chooseDevicePreset(item, realIndex)}><span className="preset-number">{presetSlotLabel(item.slot)}</span><span className="preset-name">{item.name || "Untitled preset"}</span></button>;
              })}
              {!devicePresets.length && <div className="empty-library">Connect the GE150 Pro and sync to load its presets.</div>}
            </div>
          </section>
          <section className="library-section computer-library">
            <div className="section-heading"><div><small>LOCAL FILES</small><h2>Computer</h2></div><button className="icon-button" aria-label="Add computer preset" title="Add preset" onClick={() => void addComputerPreset()}><svg aria-hidden="true" viewBox="0 0 20 20"><path d="M10 3v14M3 10h14" /></svg></button></div>
            <div className="preset-list computer-list">
              {computerPresets.map((path) => <button key={path} className={`preset-row ${path === activeComputerPath ? "selected" : ""}`} onClick={() => void loadComputerPreset(path)}><svg className="file-glyph" aria-hidden="true" viewBox="0 0 16 18"><path d="M3 1.5h6l4 4v11H3zM9 1.5v4h4M5.5 9h5M5.5 12h5" /></svg><span className="preset-name">{path.split(/[\\/]/).pop()?.replace(/\.mo$/i, "")}</span></button>)}
            </div>
            <button className="text-button" onClick={() => void addComputerPreset()}>Import .mo preset</button>
          </section>
        </aside>

        <main className="editor">
          <div className="editor-heading"><div><small>PRESET WORKSPACE</small><h1>{currentName}</h1><p>{activeComputerPath ? "Computer preset · editable copy" : selectedDevice !== null ? `Device preset ${presetSlotLabel(devicePresets[selectedDevice]?.slot ?? 1)}` : preset ? "AI-generated preset · editable copy" : "Select a device or computer preset to begin"}</p></div><div className="editor-tags"><span>GE150 PRO</span>{selectedDevice !== null ? <span>SAVE TARGET {presetSlotLabel(devicePresets[selectedDevice]?.slot ?? 1)}</span> : preset && <label className="target-picker"><small>LOAD TARGET</small><select aria-label="Pedal preset target" value={targetSlot ?? ""} onChange={(event) => { const slot = event.target.value ? Number(event.target.value) : null; setTargetSlot(slot); if (slot === null) setAiLoadOnTarget(false); }}><option value="">Choose pedal preset…</option>{devicePresets.map((item) => <option key={item.slot} value={item.slot}>{presetSlotLabel(item.slot)} · {item.name || "Untitled preset"}</option>)}</select></label>}</div></div>
          {incomplete && <div className="warning-banner">Some model parameters are not in the bundled catalog. Raw device values are shown; editing/export is disabled to avoid creating a broken preset.</div>}
          <section className="chain-section"><div className="section-title"><div><small>01 / YOUR RIG</small><h2>Signal chain</h2></div><span>SELECT A MODULE TO EDIT</span></div><div className="chain-scroll"><div className="chain">
            {BLOCKS.map((name, slot) => {
              const current = preset?.effectModule[String(slot)];
              const info = current ? bootstrap?.models[slot]?.[current.TYPE] : undefined;
              const modelName = modelDisplayName(info, slot, current?.TYPE ?? 0, customAmpNames, customCabNames, false);
              const rackModule = slot === 2 || slot === 3;
              return <button key={slot} aria-pressed={selectedBlock === slot} onClick={() => setSelectedBlock(slot)} className={`chain-module ${rackModule ? `rack-module ${slot === 3 ? "cab-rack" : "amp-rack"}` : "stomp-pedal"} ${selectedBlock === slot ? "active" : ""}`}>
                {rackModule ? <>
                  <span className="rack-screws" aria-hidden="true"><i /><i /></span>
                  <span className="rack-label">{slot === 2 ? "AMPLIFIER" : "CABINET"}</span>
                  <b>{modelName || (current ? `MODEL ${current.TYPE}` : "—")}</b>
                  <span className="rack-art" aria-hidden="true"><img src={slot === 3 ? cabinetArt : ampHeadArt} alt="" /></span>
                  <span className="pedal-state">{current?.SWITCH ? "ACTIVE" : "BYPASS"}</span>
                </> : <>
                  <span className="pedal-topline"><i className={`module-led ${current?.SWITCH ? "on" : ""}`} /><small>STOMPBOX</small></span>
                  <span className="pedal-kind">{name}</span>
                  <b>{modelName || (current ? `MODEL ${current.TYPE}` : "—")}</b>
                  <img className="pedal-art" src={pedalArt} alt="" />
                  <span className="pedal-state">{current?.SWITCH ? "ACTIVE" : "BYPASS"}</span>
                </>}
              </button>;
            })}
          </div></div></section>
          {preset ? <section className={`controls-section ${selectedBlock === 2 ? "amp-console" : "pedal-console"}`}>
            {selectedBlock === 2 && <div className="amp-header">
              <div className="amp-display"><small>AMPLIFIER MODEL</small><strong>{modelDisplayName(modelInfo, selectedBlock, module?.TYPE ?? 0, customAmpNames, customCabNames, false) || "SELECT A MODEL"}</strong></div>
              <div className="amp-power"><i className={module?.SWITCH ? "powered" : ""} /><span>{module?.SWITCH ? "POWER ON" : "STANDBY"}</span></div>
            </div>}
            <div className="section-title control-heading"><div><small>{selectedBlock === 2 ? "02 / AMP CONTROL PANEL" : `02 / EFFECTS · ${String(selectedBlock + 1).padStart(2, "0")}`}</small><h2>{BLOCKS[selectedBlock]} <span className="muted">controls</span></h2></div>{module && <label className="module-toggle"><span>{module.SWITCH ? "ACTIVE" : "BYPASSED"}</span><input type="checkbox" checked={!!module.SWITCH} onChange={(e) => updateModule(selectedBlock, { SWITCH: e.target.checked ? 1 : 0 })}/><i /></label>}</div>
            {module && <div className="model-select"><label>{selectedBlock === 2 ? "AMP MODEL" : selectedBlock === 3 ? "CABINET" : "EFFECT MODEL"}</label><select value={module.TYPE} onChange={(e) => changeModel(selectedBlock, Number(e.target.value))}>{models.map((m, index) => <option key={index} value={index} disabled={(!m.available || !m.parameters.length) && index !== module.TYPE}>{modelDisplayName(m, selectedBlock, index, customAmpNames, customCabNames)}</option>)}</select></div>}
            {modelInfo?.customIr && <div className="ir-note">AI can replace this custom IR with a catalog CAB. Preset files reference an IR slot; they do not contain the IR audio.</div>}
            <div className={`knob-grid ${selectedBlock === 2 ? "amp-knobs" : "pedal-knobs"}`}>{modelInfo?.parameters.map((parameter) => {const name=parameter.name; const value=module?.Data[name]; const eqGain=isEqGain(name); return parameter.choices.length ? <div className="choice-control" key={name}><label>{name}</label><select value={Number(value ?? parameter.default)} onChange={(e)=>updateParameter(selectedBlock,name,Number(e.target.value))}>{parameter.choices.map((choice,index)=><option value={index} key={choice}>{choice}</option>)}</select></div> : typeof value === "number" ? <Knob key={name} label={eqGain ? `${name} · dB` : name} value={value} displayValue={eqGain ? value - 16 : undefined} min={parameter.minimum} max={parameter.maximum} step={parameter.step || 1} onChange={(next) => updateParameter(selectedBlock, name, next)} /> : <div className="unmapped-control" key={name}><small>{name}</small><b>{String(value ?? "—")}</b></div>;})}</div>
            {incomplete && devicePresets[selectedDevice ?? -1]?.modules[selectedBlock] && <div className="raw-values"><small>RAW DEVICE VALUES</small><code>{devicePresets[selectedDevice!].modules[selectedBlock]?.parameters.join(" · ")}</code></div>}
          </section> : <div className="empty-editor"><span className="empty-mark">◉</span><h2>Your tone starts with a preset</h2><p>Sync the GE150 Pro library and pick any of its presets, or open a local .mo file. The AI assistant will work from the selected tone.</p></div>}
          <footer className="editor-footer"><span><i className="footer-led" />{status}</span><span>EXP settings preserved from template</span></footer>
        </main>

          <aside className="assistant panel"><div className="assistant-title"><span className="spark"><svg aria-hidden="true" viewBox="0 0 24 24"><path d="m12 2 1.7 7.1L21 12l-7.3 2.2L12 22l-2-7.8L3 12l7-2.9zM19 2v5m-2.5-2.5h5M5 17v5m-2.5-2.5h5" /></svg></span><div><small>AI DESIGNER</small><h2>Tone assistant</h2></div></div><p className="assistant-intro">Describe a tone and let AI build all nine blocks from the GE150 catalog. It can replace unmapped or custom CAB models.</p><label className="field-label">TONE BRIEF</label><textarea value={brief} onChange={(e) => setBrief(e.target.value)} placeholder="Tight modern metal rhythm, humbuckers, Drop D, studio monitors…" rows={6}/><label className={`load-on-target ${aiLoadOnTarget ? "enabled" : ""}`}><input type="checkbox" checked={aiLoadOnTarget} disabled={busy || targetSlot === null} onChange={(event) => setAiLoadOnTarget(event.target.checked)}/><span>LOAD ON TARGET</span><small>{aiLoadOnTarget ? `→ ${currentTarget ? presetSlotLabel(currentTarget.slot) : "choose target"}` : "OFF · save .MO"}</small></label><button className="button button-primary generate" disabled={busy || !(preset || template) || !brief.trim() || (aiLoadOnTarget && targetSlot === null)} onClick={() => void generateTone()}>{busy ? "WORKING…" : aiLoadOnTarget ? "GENERATE + LOAD TARGET" : "GENERATE + SAVE .MO"}</button>{summary && <div className="tone-plan"><small>AI TONE PLAN</small><p>{summary}</p></div>}
          <details className="provider-settings"><summary>Provider settings</summary><label className="field-label">API BASE URL</label><input value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} /><label className="field-label">MODEL</label><input value={model} onChange={(e) => setModel(e.target.value)} /><label className="field-label">API KEY <span>{apiKeySaved ? "SAVED IN KEYCHAIN" : "NOT SAVED"}</span></label><input type="password" value={apiKey} onChange={(e) => { setApiKey(e.target.value); setApiKeySaved(false); }} placeholder="Optional for local providers" /><button className="button button-secondary" onClick={() => void saveApiKey()}>{apiKey ? "SAVE API KEY" : "REMOVE SAVED KEY"}</button><small className="keychain-note">Stored in your operating system’s credential store, not browser storage.</small></details>
          <details className="provider-settings custom-import-settings"><summary>Custom model import</summary>
            <div className="import-tabs" role="tablist" aria-label="Custom model kind">
              <button type="button" role="tab" id="import-amp-tab" aria-controls="import-amp-panel" aria-selected={importingAmp} tabIndex={importingAmp ? 0 : -1} className={importingAmp ? "active" : ""} onClick={() => setCustomImportTab("amp")}>AMP · GNR / NAM</button>
              <button type="button" role="tab" id="import-cab-tab" aria-controls="import-cab-panel" aria-selected={importingCab} tabIndex={importingCab ? 0 : -1} className={importingCab ? "active" : ""} onClick={() => setCustomImportTab("cab")}>CAB · WAV</button>
            </div>
            <div role="tabpanel" id={importingAmp ? "import-amp-panel" : "import-cab-panel"} aria-labelledby={importingAmp ? "import-amp-tab" : "import-cab-tab"}>
              <label className="field-label custom-import-target-label">DESTINATION SLOT</label>
              {customTargets.length ? <select className="custom-import-select" value={activeImportTarget?.index ?? ""} onChange={(event) => setImportTargetIndex(Number(event.target.value))}>{customTargets.map(({ model, index }) => <option key={index} value={index}>{modelDisplayName(model, importBlock, index, customAmpNames, customCabNames, false)}</option>)}</select> : <div className="custom-import-error">No custom slots are available.</div>}
              <div className="custom-import-actions">
                {importingAmp ? <>
                  <button className="button button-secondary" disabled={busy || !devicePresets.length || !activeImportTarget} onClick={() => void importCustomModel("gnr")}>IMPORT .GNR</button>
                  <button className="button button-primary" disabled={busy || !devicePresets.length || !activeImportTarget} onClick={() => void importCustomModel("nam")}>CONVERT .NAM + IMPORT</button>
                </> : <button className="button button-primary" disabled={busy || !devicePresets.length || !activeImportTarget} onClick={() => void importCustomModel("wav")}>IMPORT .WAV IR</button>}
              </div>
              {importingAmp && <>
                <div className="converter-path"><input aria-label="NAM converter executable path" value={namConverterPath} onChange={(event) => setNamConverterPath(event.target.value)} placeholder="Path to nam_to_gnr_cli"/><button type="button" className="text-button" disabled={busy} onClick={() => void chooseNamConverter()}>BROWSE</button></div>
                <small className="custom-import-warning">The selected converter runs as your user. This app does not remove quarantine flags or change executable permissions; the proprietary CLI is not bundled.</small>
              </>}
              <small className="custom-import-warning">Upload overwrites the selected pedal slot and has no backup/restore path in this app.</small>
              {importingCab && <small className="custom-import-warning wav-limit">WAV import supports PCM 16/24/32-bit mono/stereo at 44.1/48/88.2/96 kHz. Resampling is a local approximation, not yet compared with MOOER Studio.</small>}
            </div>
          </details>
          <div className="assistant-foot">OpenAI-compatible chat completions<br/>API key protected by system credential storage.</div>
        </aside>
      </div>
      {tunerOpen && <Tuner onClose={() => setTunerOpen(false)} />}
    </div>
  );
}

function Knob({label,value,displayValue,min,max,step,onChange}:{label:string;value:number;displayValue?:number;min:number;max:number;step:number;onChange:(value:number)=>void}) {
  const ratio = max === min ? 0 : (value - min) / (max - min);
  const angle = -135 + ratio * 270;
  const shown = displayValue ?? value;
  return <div className="knob-control"><div className="knob-face" style={{"--angle":`${angle}deg`} as CSSProperties}><div className="knob-notch"/><input aria-label={label} type="range" min={min} max={max} step={step} value={value} onChange={(e)=>onChange(Number(e.target.value))}/><span className="knob-value">{Number.isInteger(shown)?shown:shown.toFixed(1)}</span></div><span className="knob-label">{label}</span></div>;
}

function modelDisplayName(model: ModelInfo | null | undefined, slot: number, index: number, ampNames: string[], cabNames: string[], showBasedOn = true) {
  const name = model?.customAmp
    ? ampNames[index - 55]?.trim() || `CUSTOM AMP ${String(index - 54).padStart(2, "0")}`
    : model?.customIr
      ? cabNames[index - 26]?.trim() || `CUSTOM IR ${String(index - 25).padStart(2, "0")}`
      : model?.name?.replace(/^\d+[: ]*/, "") || (slot === 2 ? `MODEL ${index}` : "");
  return showBasedOn && model?.basedOn ? `${name} (${model.basedOn})` : name;
}

function isEqGain(name:string) {
  return ["GAIN1","GAIN2","GAIN3","80Hz","100Hz","200Hz","240Hz","250Hz","400Hz","630Hz","750Hz","800Hz","1.6KHz","2.2KHz","3.2KHz","4KHz","6.6KHz"].includes(name);
}

function presetSlotLabel(slot:number) {
  const safeSlot = Math.max(1, Math.min(200, slot));
  return `${String(Math.floor((safeSlot - 1) / 4) + 1).padStart(2, "0")}${"ABCD"[(safeSlot - 1) % 4]}`;
}
