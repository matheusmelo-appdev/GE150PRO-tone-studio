import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import vm from "node:vm";

let Processor;
class AudioWorkletProcessor {
  constructor() { this.port = { onmessage: null, postMessage() {} }; }
}
vm.runInNewContext(await readFile(new URL("../public/recorder-processor.js", import.meta.url), "utf8"), {
  AudioWorkletProcessor,
  registerProcessor: (_name, processor) => { Processor = processor; },
  Float32Array,
});

const processor = new Processor();
const messages = [];
processor.port.postMessage = (message) => messages.push(message);
const left = Float32Array.from({ length: 128 }, (_, index) => index / 128);
const right = Float32Array.from({ length: 128 }, (_, index) => -index / 128);
const output = new Float32Array(128).fill(1);
assert.equal(processor.process([[left, right]], [[output]]), true);
processor.port.onmessage({ data: "stop" });
assert.deepEqual(Array.from(messages[0].channels[0]), Array.from(left));
assert.deepEqual(Array.from(messages[0].channels[1]), Array.from(right));
assert.equal(messages[1].type, "stopped");
assert.ok(output.every((sample) => sample === 0));
assert.equal(processor.process([[left, right]], [[output]]), false);
