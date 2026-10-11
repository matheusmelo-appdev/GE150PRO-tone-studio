# GE150 Pro editor reverse-engineering notes

Static inspection of the locally installed MOOER GE150 Pro editor. Findings below are limited to what the installed binary and connected device expose; no firmware update or preset write was sent.

## Confirmed

- Editor version is **V2.1.4** (`/Applications/GE150PRO.app`, `Contents/MacOS/config.ini`). The main executable is a universal macOS binary (arm64 and x86_64) built with Qt 6.8.3.
- The editor links both `libusb` and `hidapi` and has named code paths for USB/HID communication, preset import/export, and backup: `MyLibusb`, `usbReceiveThread`, `usbSendThread`, `CPresetFile`, and `BackupFile`.
- On this machine the connected GE150 Pro enumerates as USB VID `0x0483`, PID `0x5703` (device reports STMicroelectronics VID; manufacturer string says NXP SEMICONDUCTORS).
- The editor binary contains the preset schema markers `GE150PRO_PRESET`, `GE150PRO Preset`, `effectModule`, `preset_name`, and a `*.mo` file filter. It also has a separate `.gnr` import/export path.
- The Qt resource initializer registers an embedded `/preset/preset/preset.xml` resource. It defines model names, parameter names/ranges, and `paraPos` groups; models with an empty parameter list inherit the schema for their `paraPos`. The generator now expands those shared schemas and can build a blank preset without a tone template.
- AMP and CAB entries marked `import="1"` are user-model slots, not unsupported effects. Their numeric parameter maps are inherited through `paraPos`; uploaded names/audio are separate device data. The official parser handles response `0x41` as two arrays of 20 fixed 16-byte AMP/IR names. The receive handler requests those names with `0xC1` immediately after parsing system response `0x32`, then requests rhythm data with `0x8C`. MOOER Studio displays these custom names on the pedal (user-confirmed). Our read-only exchange still receives `0x4D` slot status and `0x0C` rhythm but no `0x41`, so our remaining gap is reproducing the complete device-session behavior—not proving whether names are pedal-backed.
- The official binary also contains `QNamConversionThread::ParseJsonLine`, `AMPForGNR::openGirFile`, and separate `ImportAmpData` / `ImportIrData` USB paths. This app does not implement NAM-to-GNR conversion or upload custom amp/IR files; the `.gnr` files in `generated/` are data examples only.
- Preset import code has distinct JSON-check, file-read, and device-write methods (`CPresetFile::CheckJsonFile`, `ReadFileToPreset`, `SetPresetWithJsonFile`). Device writes are mediated by `usbSendThread::SendPresetDataToMcu` / `SendBackupPreset`; reads include current/all preset requests.
- HID and bulk command wrappers append the table-driven CRC checksum before transfer. HID output reports are length-prefixed: each 64-byte report starts with the actual payload count (up to `0x3f`), followed by that many protocol bytes. Full reports use prefix `0x3f`; short final reports use their short count. Our USB writer now follows this framing.
- Full-library reading is verified against the connected pedal. The editor's all-preset request is an 8-byte `AA 55 02 00 A0 01 CRC-hi CRC-lo` payload sent in a 64-byte HID report whose first byte is the payload count (`0x3f` for a full report; short reports use their actual byte count). The GE150 returned 200 preset records under response command `0x20`; each has a one-byte slot, 16-byte name, nine 24-byte blocks (switch, model index, ten 16-bit values), and trailing expression data.
- The editor also has a separate current-preset request (`B0 01`) and module response commands `0x02`–`0x0A`. The app uses the full-library records for browsing, avoiding assumptions about which preset is currently active.
- The frame checksum is CRC-16/CCITT (poly `0x1021`, initial `0`), complemented and sent big-endian. The response parser validates checksums and resynchronizes on the `AA 55` frame marker.
- The UI maps raw values to catalog parameter names. Imported CAB/IR slots inherit the catalog's CAB `paraPos` schema and are preserved as references; the IR waveform itself is separate pedal data. Other unmapped blocks are shown raw, and exporting a partially mapped preset remains disabled. Expression-pedal settings are still inherited from the selected template.
- Preset selection uses command `0x96` with the one-based slot number. Live module refresh uses commands `0x82`–`0x8A` and a 24-byte module block.
- The editor's static save path sends `0xC3` with a one-based slot and the 244-byte preset body. The app builds that body from the synced device record, stores a local raw backup, writes only after confirmation, reads back the slot, and attempts automatic restore if verification fails. The write/rollback path is implemented but has not been exercised against the pedal.

## Official app: sync and custom-model evidence

The installed V2.1.4 arm64 executable was analyzed with REA/Ghidra 12.1.4. The app binary is universal; the arm64 slice analyzed here has SHA-256 `863f1390c9f7c8bad081f1d10487008ab8a26d0067f4489612acac55155a9122` (the full universal executable hash remains the fingerprint above). These are static decompiler observations, not source code or a live USB capture.

- `usbReceiveThread::ReadThread` continuously calls `HID_Read` with a 500 ms timeout and sends non-empty reads to the frame analyzer. This is a persistent receive loop, unlike a one-shot request/read transaction.
- `usbReceiveThread::DealData` handles response `0x32` by parsing system data, then—when its name-query flag is set—calling `RequestIrAmpName` and `RequestRhythmData` in that order. This explains why sending `0xC1` alone or batching it before the system response did not reproduce the editor workflow.
- The statically constructed requests are eight bytes: system data `AA 55 02 00 31 01 34 12`, all presets `AA 55 02 00 A0 01 1F C8`, custom names `AA 55 02 00 C1 01 27 D3`, and rhythm `AA 55 02 00 8C 01 5C 43`. REA shows the official startup sends system data then presets; the receive callback sends `0xC1`/`0x8C` as soon as `0x32` arrives. The name request constant is `READ_CUSTOM_MODEL_NAMES` (0xC1).
- Response `0x41` is decoded into two 20-item name lists. Each item is limited to 16 bytes; the two name tables are 320 bytes apart in the receive buffer. The app labels the second list `IRNames`, so the first is the AMP list and the second the IR/CAB list. Core's model-list code reads these arrays for custom slots. Response `0x4D` is parsed separately as slot status and is not the name table.
- The user confirms MOOER Studio displays `Hellwin SYN100-H` (AMP) and `R5_412_V30_Edgy` (CAB/IR). The official importer derives names from `QFileInfo::completeBaseName`, truncates to 16 characters, and sends the name in the model-import metadata packet. The Hellwin NAM filename in Downloads begins with exactly `Hellwin SYN100-H`; this supports it as the source label. The original R5 WAV/GIR is not present in the searched Downloads/Documents/Desktop paths. Our app remembers names for models it imports; existing names still need the successful device-side `0x41` read.
- Hardware read-only verification: `0x31` alone returned no data within the 45-second read window; requesting `0x31` plus `0xA0` returned 200 presets and system response `0x32` (also `0x12` and `0x11`). We now send `0xC1`/`0x8C` immediately from the `0x32` frame callback, using MOOER's length-prefixed HID reports. The pedal returned `0x4D` (41-byte body) and `0x0C` (7-byte body), but no `0x41`, even with a 10-second listen. The production sync returns 200 presets; the name-query mismatch remains open.
- AMP upload target IDs are zero-based custom-slot indices `0..19` (the official UI model IDs `0x37..0x4a` are translated by subtracting `0x37`; `0x36` is a special target). `ImportAmpData` reads a 10,240-byte sample from GNR, sends a 16-byte basename metadata packet, then 20 512-byte chunks. It waits up to five seconds for each `0x15` acknowledgement, with metadata using acknowledgement index 20 and data chunks 0..19.
- CAB/IR upload translates the UI model IDs `0x1a..0x2d` to custom slot indices `0..19`. The official code sends five `0xB5` packets: initialization, a 16-byte basename, then three 512-byte data packets (indices 2..4) paced 100 ms apart. `CWavFile::GetIRData` outputs 512 24-bit taps at 44.1 kHz after format conversion/resampling; our first WAV implementation uses linear resampling and is not sample-equivalent to MOOER's DSP path.
- The official helper is at `/Applications/GE150PRO.app/Contents/MacOS/GE150PRO.app/Contents/MacOS/nam/nam_to_gnr_cli`. Its CLI requires `--input <file.nam> --output <file.gnr>`, verifies extensions and output directory, and emits JSON progress records. A local conversion of `Hellwin SYN100-H Lead 2 (Knobs at Noon).nam` produced a GNR file byte-for-byte identical to `generated/hellwin_syn100h_lead2.gnr` (SHA-256 `ddd40bf96dbb84f57a931fd227d6a4dd12b5b0509d6ff59491da78e7d76a9d0a`). It emits a `mooerge` container with an `info` chunk and a 10,240-byte `data` chunk. The converter is MOOER-signed/proprietary; the app asks the user to select their own executable and does not bundle it. The official app launches it as a child process and removes its quarantine attribute; our app does not change quarantine/security attributes.
- Our GNR parser validates the outer length, `info`/`data` chunks, and exact 10,240-byte sample body. The packet builder follows the decompiled `ImportAmpData` format: `0xD3` metadata (target index, `0x14`, 16-byte filename) plus 20 `0xD3` data packets (target index, chunk index, 512 bytes). The custom slot index is `0..19`; the editor model IDs are translated by subtracting `0x37` for AMP and `0x1A` for CAB.
- The initial WAV importer validates RIFF/WAVE PCM16/24/32 mono/stereo at the official sample rates, averages stereo, linearly resamples to 44.1 kHz, and emits 512 24-bit taps into the official five-packet `0xB5` sequence (init, 16-byte basename, three data chunks). This is a deliberately limited approximation of `CWavFile::GetIRData`, not a reproduction of MOOER's DSP resampler. The user reports a CAB IR upload works; audio equivalence is still not measured.

REA Evidence (same installed app slice):

| Evidence ID | Observation |
| --- | --- |
| `ev_76995b07de7429b3bf915a4f20f25fa5a2c55173f37780dc1e5a849535a9c201` | Ghidra overview: arm64 Mach-O, 17,493 procedures and 52,375 strings |
| `ev_8a6f1a44168b1624a242372e4fe60c1eae89844eadff411a246143be85692203` | Persistent HID receive loop |
| `ev_b085638a1aa49bea5ff5eab98710c9d07595b652e18790918174c635c0bff6` | All-preset request construction |
| `ev_79ce1b024c6fbad479d6fa52aba6dd261fc1d44c54de4038aafda95db8a6fdbc` | Custom-name request construction |
| `ev_6e762f7d4a611dc8ac72af4387a35ea39e360dff59b6c17b4462ba265ed3d417` | Rhythm request construction |
| `ev_dc0783a4d82eca437a6adfe91bdd8d705b5ce51bf3751515b5cadd7f7c1b968a` | Response dispatcher: `0x32`, `0x41`, and `0x4D` handling |
| `ev_91d35b366f7cdd49c0f743b59439d8c22d579a555489ab29207a14a7cc68c3db` | AMP/GNR import path |
| `ev_e5c3908ed9cee74a74c6c0d26b6ccfa2ffd7c64bcefe4a0fe21ab1b4623e811b` | CAB/WAV import path |
| `ev_88b3e0c7f4ab1e79153af43e6188a66ccf5a8f472aadd311b3fb8b0dab168fe5` | AMP custom-slot transfer with basename and 512-byte data blocks |
| `ev_c48b13702a05d460e8cb95e509d4eb8b5b0933be7f418baf929188d209bbe74b` | CAB transfer, basename handling, and WAV/GIR branch |
| `ev_d5bcf764308da89e107768454d8ce936f66c2d79d46388fea8b93de3d87028e8` | HID write framing and CRC |
| `ev_e4d327632cf37d02be711df62d51cdcfbe39897e790a5ee32eb7e586ada6a3008` | Device responses routed to the core UI |
| `ev_655e2c8d16c3d1a9c49183557d4b7112ae627337508bd670cd35153c2f94018b` | NAM CLI argument validation (`--input`, `--output`) |
| `ev_e665204397b86b4189fb0f79b8d063382ac147df62c0967ed1716b0070a5a3eb` | NAM input/output extension and directory validation |
| `ev_91d35b366f7cdd49c0f743b59439d8c22d579a555489ab29207a14a7cc68c3db` | GNR upload packet path and 16-byte basename handling |
| `ev_c48b13702a05d460e8cb95e509d4eb8b5b0933be7f418baf929188d209bbe74b` | IR upload packet path and 16-byte basename handling |

## Not yet established

- Expression-pedal data and all model parameter schemas.
- Why our HID read receives `0x4D` but not the `0x41` custom-name table that MOOER Studio displays. The names are pedal-backed per the user's verification and the official parser; our exact USB session still differs from MOOER's successful read.
- User reports CAB WAV upload succeeds and the AMP appears in the pedal's list. The first NAM-converted GNR upload timed out waiting for metadata ACK (`0x15`, block 20). The cause was our ACK matcher using catalog ID `0x37 + slot` while packet and pedal ACK use zero-based `slot`; the matcher is now corrected. The first attempt sent no sample blocks, though the slot name may have changed. The AMP playback/updated ACK path has not been verified; retry only on an expendable custom slot with MOOER Studio closed.
- Physical verification of live module updates, `0xC3` save/commit behavior, and automatic rollback on this pedal/firmware.
- Whether the current JSON `.mo` schema can be written directly over USB or the editor converts it to a separate binary device representation first.
- Whether some firmware revisions need an additional save/commit command after `0xC3`.
- Firmware container internals. The bundled firmware file was identified by path and hash only; it was not modified or analyzed.

## Implementation boundary

The app supports full preset-library sync, live slot selection, live module updates, guarded preset writes, a USB-audio tuner, and explicit custom-model imports. Preset saves have backup/read-back verification; custom-model imports overwrite a selected model slot and do not have a backup/restore path. The user reports the tuner and IR upload work; AMP GNR playback after the ACK target fix remains unverified. Never send firmware-update commands from the tone editor.

## Local binary fingerprint

- Executable SHA-256: `28c0d77ea1fd8d3349cef1e46ca7af93992c17d59cebf95517c538c668d27de7`
- Bundled firmware SHA-256 (not analyzed): `4ffd2b6aa7349444d7e4c3adc06d933f0e412282511a32675991f520994e522a`

## Windows package cross-check

- `GE150_Pro_V2.1.4_Win.zip` in Downloads contains `GE150PRO_V2.1.4_Setup_Win.exe` and a firmware-update HTML note; it does not contain the installed editor files directly.
- The installer identifies as an Inno Setup 6.7.0 executable. `innoextract` 1.9 reports an unexpected loader revision/checksum mismatch and cannot list or extract its payload. The installer was not executed. No Windows editor code has been inspected yet.
