export type AudioClip = { channels: Float32Array[]; sampleRate: number; frames: number };

export function makeWav(clip: AudioClip, start: number, end: number) {
  const channelCount = Math.min(2, clip.channels.length);
  const firstFrame = Math.max(0, Math.floor(start * clip.sampleRate));
  const lastFrame = Math.min(clip.frames, Math.floor(end * clip.sampleRate));
  const frames = Math.max(0, lastFrame - firstFrame);
  const bytes = new ArrayBuffer(44 + frames * channelCount * 2);
  const view = new DataView(bytes);
  const text = (offset: number, value: string) => [...value].forEach((char, index) => view.setUint8(offset + index, char.charCodeAt(0)));
  text(0, "RIFF"); view.setUint32(4, bytes.byteLength - 8, true); text(8, "WAVE");
  text(12, "fmt "); view.setUint32(16, 16, true); view.setUint16(20, 1, true);
  view.setUint16(22, channelCount, true); view.setUint32(24, clip.sampleRate, true);
  view.setUint32(28, clip.sampleRate * channelCount * 2, true); view.setUint16(32, channelCount * 2, true);
  view.setUint16(34, 16, true); text(36, "data"); view.setUint32(40, frames * channelCount * 2, true);
  let offset = 44;
  for (let frame = firstFrame; frame < lastFrame; frame++) {
    for (let channel = 0; channel < channelCount; channel++) {
      const sample = Math.max(-1, Math.min(1, clip.channels[channel][frame]));
      view.setInt16(offset, sample < 0 ? sample * 32768 : sample * 32767, true);
      offset += 2;
    }
  }
  return new Uint8Array(bytes);
}
