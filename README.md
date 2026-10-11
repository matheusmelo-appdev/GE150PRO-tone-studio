# GE150 Tone

A Tauri/React desktop editor with a Rust backend for the GE150 Pro. Generate fresh `.mo` tones from a description—with AI choosing all nine block models and settings—or load a preset to edit. Browse pedal presets, tune from the GE150Pro USB audio input, import user AMP/CAB models, tweak controls, and save tones with backup and read-back verification.

## Run it

Prerequisites: Rust/Cargo, Node.js/npm, and an OpenAI-compatible chat-completions endpoint. With Ollama running locally:

```sh
ollama pull qwen2.5:7b
npm install
npm run tauri -- dev
```

The app syncs the device library on launch and lists computer presets separately. Presets display as banks of four (`01A`–`50D`); it does not load a sample tone. Enter a brief to generate from the GE150 catalog, or choose a pedal/local preset to edit. Provider URL/model are configurable; use **Save API Key** to store the key in the operating system credential store. Close MOOER Studio before USB sync.

The CLI remains available:

```sh
cargo run -- --prompt "warm jazz clean tone" --output "generated/warm jazz.mo" \
  --base-url https://api.openai.com/v1 --model gpt-4o-mini --api-key "$OPENAI_API_KEY"
```

Build a distributable app with `npm run tauri -- build`. Startup covers the editor until all 200 device preset slots have synced; if sync fails, retry is required (there is no offline bypass). After the first successful sync, the app asks whether **LIVE PEDAL** should be the remembered default. With it enabled, selecting a pedal preset activates it and knob edits sync. Computer presets can select a pedal target and load there with backup/read-back verification. AI has a separate **LOAD ON TARGET** toggle; off, generation saves a `.mo` file, and on, it backs up and writes the selected target. The Rust CLI remains available with `cargo run -- --prompt ...`.

The generated `.mo` can be dragged from its output folder into the GE150 editor's COMPUTER list, then from COMPUTER to a pedal preset slot.

## First-version limits

- With no preset loaded, AI selects block models and settings across the nine fixed GE150 slots. Existing presets are optional starting points.
- The built-in chromatic tuner listens to the connected GE150Pro USB audio input (A4=440 Hz); it does not trigger the pedal's A+B tuner mode. Microphone permission is requested only when you start it. Audio stays local. The user confirmed it works with the pedal.
- The UI shows documented factory model inspirations in parentheses, and AI receives those `based on` notes as sound-design hints. These are official manual references, not guarantees of exact hardware equivalence.
- Provider support uses the OpenAI-compatible Chat Completions API; providers with a different API protocol are not supported yet. Song names/descriptions are interpreted from the model's knowledge; AI generation does not listen to or analyze audio.
- Generated files use the editor's JSON preset schema. Import and playback should be checked with the editor/pedal before relying on a tone live.
- The catalog is extracted from the editor and resolves shared `paraPos` schemas. Imported/empty slots without a defined schema are excluded from AI model selection. Fresh tones use neutral defaults; pedal reads preserve EXP from a base preset.
- Imported AMP/CAB slots use their catalog parameter schemas, so those presets can be edited and exported instead of being marked broken. AI may replace imported models with catalog models. `.mo` presets reference custom slots; custom audio is uploaded separately.
- Custom AMP import accepts `.gnr` or `.nam`. For NAM, select the official `nam_to_gnr_cli` from your installed MOOER editor; the proprietary converter is run locally and is not bundled or redistributed. Custom IR import accepts PCM WAV files. The app remembers the selected file basename (max 16 bytes) as a local fallback name after import.
- Custom AMP/CAB uploads are in the closed **Custom model import** section beside Provider settings, with separate AMP and CAB tabs. They require explicit confirmation because they overwrite a pedal slot and cannot be backed up by this app. The user reports IR upload works and the AMP appears in the pedal's list. WAV resampling is linear and not sample-matched to MOOER Studio; AMP audio playback and the fixed ACK path still need verification on an expendable slot.
- Direct selection, live updates, and backed-up slot writes are implemented but have not been physically tested. Test on a nonessential slot first; firmware updates are not supported.

## Documentation order

1. This README — install, run, and feature status.
2. [`docs/preset-format.md`](docs/preset-format.md) — `.mo` structure and decoded sample.
3. [`docs/reverse-engineering.md`](docs/reverse-engineering.md) — USB protocol, official-app findings, import caveats, and verification status.
