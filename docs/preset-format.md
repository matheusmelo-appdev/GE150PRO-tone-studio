# GE150 PRO preset sample

Sample: [`SYN Rhythm.mo`](../samples/SYN%20Rhythm.mo), exported from the editor.

## File structure

The `.mo` file is ordinary UTF-8 JSON, not an opaque binary. It has three top-level keys:

- `Exp`: expression pedal and footswitch configuration.
- `effectModule`: nine fixed processing slots. Each slot has a `TYPE`, an on/off `SWITCH`, and a `Data` object containing that model's parameter values.
- `fileInfo`: editor/device schema and version metadata, plus the preset name.

Each `effectModule` key (`"0"` through `"8"`) is a chain slot. `TYPE` is a zero-based index into that slot's model list in `catalog/effects_en.xml`. The following decoded models match the factory preset sample:

| Slot | Block | TYPE | Model | On |
|---:|---|---:|---|:---:|
| 0 | FX/COMP | 11 | 12: INTEL REDUCER | No |
| 1 | DS/OD | 1 | 02: 808 | Yes |
| 2 | AMP | 7 | 08: J900 | Yes |
| 3 | CAB | 5 | 06: 1960 412 | Yes |
| 4 | NS GATE | 2 | 03: NOISE GATE | Yes |
| 5 | EQ | 3 | 04: CUSTOM EQ | Yes |
| 6 | MOD | 4 | 05: JET-FLANGER | No |
| 7 | DELAY | 8 | 09: DUAL DELAY | No |
| 8 | REVERB | 4 | 05: SPRING | No |

## Sample values

- 808: `VOLUME=85`, `TONE=62`, `GAIN=10`
- J900: `GAIN=44`, `BASS=50`, `MID=45`, `TREBLE=65`, `PRES=60`, `MST=93`
- 1960 412 cab: `CENTER=10`, `DISTANCE=50`, `MIC=6`, `TUBE=1`
- Noise gate: `ATTACK=100`, `RELEASE=100`, `THRES=44`
- Custom EQ: `FREQ1=398`, `FREQ2=1997`, `FREQ3=6771`, `GAIN1=13`, `GAIN2=19`, `GAIN3=18`

## Current generator behavior

The Rust CLI starts from an exported `.mo` file, asks a local Ollama model to set the nine switches and the sample's existing parameter keys, validates the response, and writes a new `.mo`. It retains the original effect model indices and chain order. Numeric values are bounded with basic ranges for common controls, frequencies, times, and cab selectors. Unrecognized categorical settings are retained from the template.

The effect XML identifies many parameter names but leaves many models without parameter definitions. For now, start from a preset whose amp and effects you want to keep. Successful import and pedal playback of a generated file still need to be confirmed. Preserve a pedal backup before testing imported presets.
