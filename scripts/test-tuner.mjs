import assert from "node:assert/strict";
import { detectPitchHz, noteForFrequency } from "../src-ui/pitch-detection.ts";

const sampleRate = 48_000;
for (const expected of [82.41, 110, 440, 880]) {
  const samples = Float32Array.from({ length: 8192 }, (_, index) =>
    Math.sin(2 * Math.PI * expected * index / sampleRate),
  );
  const detected = detectPitchHz(samples, sampleRate);
  assert.ok(detected !== null && Math.abs(detected - expected) < 1.5, `${expected} Hz detected as ${detected}`);
}
assert.equal(detectPitchHz(new Float32Array(8192), sampleRate), null);
assert.equal(noteForFrequency(440).name, "A4");
assert.ok(Math.abs(noteForFrequency(440).cents) < 0.01);
