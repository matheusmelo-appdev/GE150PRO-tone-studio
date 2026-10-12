import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm, save } from "@tauri-apps/plugin-dialog";
import { writeFile } from "@tauri-apps/plugin-fs";
import Tuner from "./TunerModal";
import { makeWav } from "./recording-audio";
import tunerIcon from "./assets/tuner.svg?url";

// shortcut: recordings stay in memory and are capped at 10 minutes; use streamed file capture for longer takes.
const MAX_TAKE_SECONDS = 600;

type Clip = { channels: Float32Array[]; sampleRate: number; frames: number };
type Capture = { stream: MediaStream; context: AudioContext; node: AudioWorkletNode; chunks: Float32Array[][]; frames: number };

type Props = { onClose: () => void; bpm: number; bpmInput: string; metronomeOn: boolean; onBpmChange: (value: number) => void; onBpmInputChange: (value: string) => void; onToggleMetronome: () => void };

export default function Recorder({ onClose, bpm, bpmInput, metronomeOn, onBpmChange, onBpmInputChange, onToggleMetronome }: Props) {
  const [clip, setClip] = useState<Clip | null>(null);
  const [recording, setRecording] = useState(false);
  const [finalizing, setFinalizing] = useState(false);
  const [starting, setStarting] = useState(false);
  const [error, setError] = useState("");
  const [tunerOpen, setTunerOpen] = useState(false);
  const [elapsed, setElapsed] = useState(0);
  const [playPosition, setPlayPosition] = useState(0);
  const [playing, setPlaying] = useState(false);
  const [trimStart, setTrimStart] = useState(0);
  const [trimEnd, setTrimEnd] = useState(0);
  const [inputName, setInputName] = useState("GE150Pro USB input");
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const captureRef = useRef<Capture | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const playbackRef = useRef<{ context: AudioContext; source: AudioBufferSourceNode } | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  const duration = clip ? clip.frames / clip.sampleRate : 0;
  const displayTime = (seconds: number) => `${Math.floor(seconds / 60)}:${String(Math.floor(seconds % 60)).padStart(2, "0")}`;

  function stopPlayback() {
    const playback = playbackRef.current;
    if (!playback) return;
    playbackRef.current = null;
    playback.source.onended = null;
    try { playback.source.stop(); } catch { /* already stopped */ }
    void playback.context.close();
    setPlaying(false);
  }

  function cleanupCapture(capture: Capture) {
    capture.stream.getTracks().forEach((track) => track.stop());
    void capture.context.close();
    if (captureRef.current === capture) captureRef.current = null;
  }

  async function startRecording() {
    if (starting || recording || finalizing) return;
    if (clip && !await confirm("Replace the current recording with a new take?", { title: "Record new take", kind: "warning", okLabel: "Replace take", cancelLabel: "Keep recording" })) return;
    setStarting(true);
    setError("");
    let stream: MediaStream | null = null;
    let context: AudioContext | null = null;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: { echoCancellation: false, noiseSuppression: false, autoGainControl: false, channelCount: 2 }, video: false });
      const inputs = (await navigator.mediaDevices.enumerateDevices()).filter((device) => device.kind === "audioinput");
      const pedal = inputs.find((device) => /ge\s*150/i.test(device.label));
      if (!pedal) throw new Error("GE150Pro is not available as a USB audio input. Reconnect it and check macOS Sound input settings.");
      const currentId = stream.getAudioTracks()[0]?.getSettings().deviceId;
      if (pedal.deviceId !== currentId) {
        stream.getTracks().forEach((track) => track.stop());
        stream = await navigator.mediaDevices.getUserMedia({ audio: { deviceId: { exact: pedal.deviceId }, echoCancellation: false, noiseSuppression: false, autoGainControl: false, channelCount: 2 }, video: false });
      }
      setInputName(pedal.label || "GE150Pro USB input");
      context = new AudioContext();
      await context.resume();
      await context.audioWorklet.addModule("/recorder-processor.js");
      const source = context.createMediaStreamSource(stream);
      const node = new AudioWorkletNode(context, "ge150-recorder", { numberOfInputs: 1, numberOfOutputs: 1, outputChannelCount: [1], channelCount: 2, channelCountMode: "max" });
      const silent = context.createGain();
      silent.gain.value = 0;
      source.connect(node).connect(silent).connect(context.destination);
      const capture: Capture = { stream, context, node, chunks: [], frames: 0 };
      streamRef.current = stream;
      captureRef.current = capture;
      stream.getAudioTracks()[0]?.addEventListener("ended", () => {
        if (captureRef.current === capture) {
          setError("GE150Pro audio disconnected; recording stopped.");
          stopRecording();
        }
      }, { once: true });
      node.port.onmessage = ({ data }: MessageEvent<{ type: string; channels?: Float32Array[] }>) => {
        if (data.type === "samples" && data.channels) {
          capture.chunks.push(data.channels);
          capture.frames += data.channels[0]?.length ?? 0;
          setElapsed(capture.frames / context!.sampleRate);
          if (capture.frames >= context!.sampleRate * MAX_TAKE_SECONDS && captureRef.current === capture) {
            setError("10-minute take limit reached.");
            stopRecording();
          }
        } else if (data.type === "stopped") {
          if (!capture.frames) {
            cleanupCapture(capture);
            setError("No audio was captured. Check the GE150Pro signal and try again.");
            setFinalizing(false);
            return;
          }
          const channelCount = Math.min(2, capture.chunks[0]?.length ?? 1);
          const channels = Array.from({ length: channelCount }, (_, channel) => {
            const samples = new Float32Array(capture.frames);
            let offset = 0;
            for (const chunk of capture.chunks) { samples.set(chunk[channel] ?? chunk[0], offset); offset += chunk[0]?.length ?? 0; }
            return samples;
          });
          const result = { channels, sampleRate: capture.context.sampleRate, frames: capture.frames };
          cleanupCapture(capture);
          setClip(result);
          setSaved(false);
          setTrimStart(0);
          setTrimEnd(result.frames / result.sampleRate);
          setElapsed(result.frames / result.sampleRate);
          setFinalizing(false);
        }
      };
      setClip(null);
      setSaved(false);
      setElapsed(0);
      setRecording(true);
    } catch (cause) {
      stream?.getTracks().forEach((track) => track.stop());
      if (context && context.state !== "closed") await context.close();
      setError(cause instanceof DOMException && cause.name === "NotAllowedError"
        ? "Microphone access was denied. Allow GE150 Tone Studio in macOS Privacy & Security, then try again."
        : cause instanceof Error ? cause.message : String(cause));
    } finally {
      setStarting(false);
    }
  }

  function stopRecording() {
    if (!captureRef.current) return;
    setRecording(false);
    setFinalizing(true);
    captureRef.current.node.port.postMessage("stop");
  }

  function playClip() {
    if (!clip || playing || recording || finalizing) return;
    const first = Math.floor(trimStart * clip.sampleRate);
    const last = Math.min(clip.frames, Math.floor(trimEnd * clip.sampleRate));
    if (last <= first) return;
    const context = new AudioContext();
    const buffer = context.createBuffer(clip.channels.length, last - first, clip.sampleRate);
    clip.channels.forEach((samples, channel) => buffer.copyToChannel(samples.subarray(first, last), channel));
    const source = context.createBufferSource();
    source.buffer = buffer;
    source.connect(context.destination);
    source.onended = () => {
      if (playbackRef.current?.source === source) {
        playbackRef.current = null;
        setPlaying(false);
        setPlayPosition(trimStart);
        void context.close();
      }
    };
    playbackRef.current = { context, source };
    setPlayPosition(trimStart);
    setPlaying(true);
    source.start();
    const startedAt = performance.now();
    const advance = () => {
      if (playbackRef.current?.source !== source) return;
      const position = trimStart + (performance.now() - startedAt) / 1000;
      setPlayPosition(Math.min(position, trimEnd));
      if (position < trimEnd) requestAnimationFrame(advance);
    };
    requestAnimationFrame(advance);
  }

  async function exportWav() {
    if (!clip || saving) return;
    const data = makeWav(clip, trimStart, trimEnd);
    if (data.byteLength <= 44) return;
    const path = await save({ defaultPath: "GE150 recording.wav", filters: [{ name: "WAV audio", extensions: ["wav"] }] });
    if (!path) return;
    setSaving(true);
    try {
      await invoke("allow_recording_output", { path });
      await writeFile(path, data);
      setSaved(true);
      setError("");
    } catch (cause) {
      setError(`Could not save WAV · ${String(cause)}`);
    } finally {
      setSaving(false);
    }
  }

  async function closeRecorder() {
    if (clip && !saved && !await confirm("Discard the unsaved recording?", { title: "Unsaved recording", kind: "warning", okLabel: "Discard", cancelLabel: "Keep recording" })) return;
    stopPlayback();
    onClose();
  }

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas || !clip) return;
    const context = canvas.getContext("2d");
    if (!context) return;
    const width = canvas.width;
    const height = canvas.height;
    context.clearRect(0, 0, width, height);
    context.fillStyle = "#171512";
    context.fillRect(0, 0, width, height);
    const startFrame = 0;
    const endFrame = clip.frames;
    const samples = clip.channels[0];
    const step = Math.max(1, Math.floor((endFrame - startFrame) / width));
    context.strokeStyle = "#e8a95f";
    context.beginPath();
    for (let x = 0; x < width; x++) {
      const from = startFrame + x * step;
      if (from >= endFrame) break;
      let peak = 0;
      for (let index = from; index < Math.min(from + step, endFrame); index++) peak = Math.max(peak, Math.abs(samples[index]));
      const extent = Math.max(1, peak * height * 0.46);
      context.moveTo(x + 0.5, height / 2 - extent);
      context.lineTo(x + 0.5, height / 2 + extent);
    }
    context.stroke();
    const trimX = Math.round((trimStart / duration) * width);
    const trimEndX = Math.round((trimEnd / duration) * width);
    context.fillStyle = "#05050599";
    context.fillRect(0, 0, trimX, height);
    context.fillRect(trimEndX, 0, width - trimEndX, height);
    const playX = Math.round((playPosition / duration) * width);
    if (playing) {
      context.strokeStyle = "#f2e8d4";
      context.beginPath(); context.moveTo(playX, 0); context.lineTo(playX, height); context.stroke();
    }
  }, [clip, trimStart, trimEnd, playPosition, playing, duration]);

  useEffect(() => {
    return () => {
      if (captureRef.current) cleanupCapture(captureRef.current);
      if (playbackRef.current) {
        playbackRef.current.source.onended = null;
        try { playbackRef.current.source.stop(); } catch { /* already stopped */ }
        void playbackRef.current.context.close();
      }
      streamRef.current?.getTracks().forEach((track) => track.stop());
    };
  }, []);

  return <div className="recorder-backdrop">
    <section className="recorder-screen" role="dialog" aria-modal="true" aria-labelledby="recorder-title">
      <header className="recorder-header"><div><small>GE150 PRO · USB AUDIO</small><h1 id="recorder-title">Guitar recorder</h1></div><div className="recorder-tools"><button className="button button-secondary recorder-icon-button" aria-label="Open tuner" title="Tuner" onClick={() => setTunerOpen(true)}><img src={tunerIcon} alt="" /></button><button className="button button-secondary" aria-pressed={metronomeOn} onClick={onToggleMetronome}>{metronomeOn ? "STOP CLICK" : `METRONOME · ${bpm} BPM`}</button><button className="icon-button tuner-close" aria-label="Close recorder" disabled={recording || finalizing || saving} onClick={() => void closeRecorder()}>×</button></div></header>
      <div className="recorder-body">
        <section className="metro-panel"><div><small>CLICK TEMPO</small><b>{bpm} <span>BPM</span></b></div><label className="metro-slider"><span className="sr-only">Metronome tempo</span><input type="range" min="40" max="240" value={bpm} onChange={(event) => onBpmChange(Number(event.target.value))}/><input className="metro-number" type="number" min="40" max="240" value={bpmInput} aria-label="Tempo in beats per minute" onChange={(event) => onBpmInputChange(event.target.value)} onBlur={() => onBpmInputChange(String(bpm))}/></label><small className="metro-note">Quarter-note click · downbeat accented · not recorded</small></section>
        <div className="record-status"><span className={recording ? "recording-dot live" : "recording-dot"}/>{recording ? "RECORDING" : finalizing ? "FINALIZING TAKE…" : playing ? "PLAYING" : clip ? "TAKE READY" : "READY TO RECORD"}<b>{displayTime(recording ? elapsed : clip ? duration : 0)}</b></div>
        <section className="track-panel"><div className="track-label"><span>01</span><div><b>GE150 GUITAR</b><small>{inputName} · {clip ? `${clip.sampleRate / 1000} kHz · ${clip.channels.length === 1 ? "MONO" : "STEREO"}` : "single track"}</small></div><button className="button button-secondary" disabled={!clip || recording || finalizing || playing || saving} onClick={() => void exportWav()}>{saving ? "SAVING…" : saved ? "EXPORTED WAV" : "EXPORT WAV"}</button></div>
          <canvas ref={canvasRef} className="waveform" width={960} height={160} aria-label={clip ? "Recorded guitar waveform" : "Empty recording waveform"}/>
          <div className="track-empty">{clip ? `TRIM SELECTION · ${displayTime(trimStart)} — ${displayTime(trimEnd)}` : "Your recording waveform will appear here"}</div>
        </section>
        <section className="trim-panel"><label>IN <b>{displayTime(trimStart)}</b><input type="range" min="0" max={duration || 1} step="0.01" value={Math.min(trimStart, duration || 1)} disabled={!clip || recording || finalizing} onChange={(event) => { const next = Number(event.target.value); setTrimStart(Math.min(next, trimEnd - 0.01)); setSaved(false); }}/></label><label>OUT <b>{displayTime(trimEnd)}</b><input type="range" min="0" max={duration || 1} step="0.01" value={Math.min(trimEnd, duration || 1)} disabled={!clip || recording || finalizing} onChange={(event) => { setTrimEnd(Math.max(Number(event.target.value), trimStart + 0.01)); setSaved(false); }}/></label><small>Trim in/out points to edit the saved section. Original take stays intact until a new recording replaces it.</small></section>
        {error && <div className="tuner-error" role="alert">{error}</div>}
        <footer className="recorder-transport"><div className="transport-left"><button className="button button-secondary" disabled={!clip || recording || finalizing || !playing} onClick={stopPlayback}>STOP</button><button className="button button-secondary" disabled={!clip || recording || finalizing || playing} onClick={playClip}>PLAY SELECTION</button></div><div className="transport-main">{recording ? <button className="button button-record stop-record" aria-label="Stop recording" title="Stop recording" onClick={stopRecording}><i className="record-stop-icon" aria-hidden="true"/></button> : <button className="button button-record" aria-label={clip ? "Record a new take" : "Start recording"} title={starting ? "Connecting to GE150Pro" : clip ? "Record a new take" : "Start recording"} disabled={starting || finalizing || playing} onClick={() => void startRecording()}><i className="record-start-icon" aria-hidden="true"/></button>}</div><small>Single track · maximum 10-minute take</small></footer>
      </div>
    </section>
    {tunerOpen && <Tuner onClose={() => setTunerOpen(false)} />}
  </div>;
}
