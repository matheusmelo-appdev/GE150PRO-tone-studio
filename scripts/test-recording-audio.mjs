import assert from "node:assert/strict";
import { makeWav } from "../src-ui/recording-audio.ts";

const clip = {
  channels: [Float32Array.from([-1, 0, 1, 0.5]), Float32Array.from([1, 0, -1, -0.5])],
  sampleRate: 4,
  frames: 4,
};
const wav = makeWav(clip, 0.25, 0.75);
const view = new DataView(wav.buffer);
assert.equal(new TextDecoder().decode(wav.subarray(0, 4)), "RIFF");
assert.equal(new TextDecoder().decode(wav.subarray(8, 12)), "WAVE");
assert.equal(view.getUint32(24, true), 4);
assert.equal(view.getUint16(22, true), 2);
assert.equal(view.getUint32(40, true), 8);
assert.equal(view.getInt16(44, true), 0);
assert.equal(view.getInt16(46, true), 0);

const mono = makeWav({ channels: [Float32Array.from([-1, 1])], sampleRate: 8, frames: 2 }, 0, 0.25);
assert.equal(new DataView(mono.buffer).getUint16(22, true), 1);
assert.equal(new DataView(mono.buffer).getInt16(44, true), -32768);
