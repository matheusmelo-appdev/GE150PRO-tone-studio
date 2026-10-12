class Ge150RecorderProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.channels = [];
    this.frames = 0;
    this.chunkSize = 8192;
    this.stopped = false;
    this.port.onmessage = ({ data }) => {
      if (data === "stop") {
        this.flush();
        this.stopped = true;
        this.port.postMessage({ type: "stopped" });
      }
    };
  }

  flush() {
    if (!this.frames) return;
    const channels = this.channels.map((buffer) => buffer.slice(0, this.frames));
    this.port.postMessage({ type: "samples", channels }, channels.map((channel) => channel.buffer));
    this.channels = [];
    this.frames = 0;
  }

  process(inputs, outputs) {
    const input = inputs[0];
    const output = outputs[0];
    for (const channel of output) channel.fill(0);
    if (this.stopped) return false;
    if (!input?.length) return true;
    if (this.channels.length !== input.length) {
      this.flush();
      this.channels = input.map(() => new Float32Array(this.chunkSize));
      this.frames = 0;
    }
    const frames = input[0].length;
    let offset = 0;
    while (offset < frames) {
      const count = Math.min(this.chunkSize - this.frames, frames - offset);
      for (let channel = 0; channel < input.length; channel++) {
        this.channels[channel].set(input[channel].subarray(offset, offset + count), this.frames);
      }
      this.frames += count;
      offset += count;
      if (this.frames === this.chunkSize) this.flush();
    }
    return true;
  }
}

registerProcessor("ge150-recorder", Ge150RecorderProcessor);
