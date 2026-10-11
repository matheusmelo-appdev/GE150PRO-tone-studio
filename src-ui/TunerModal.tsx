import { useEffect, useRef, useState } from "react";
import { detectPitchHz, noteForFrequency } from "./pitch-detection";

type Props = { onClose: () => void };
type Reading = { note: string; cents: number; frequency: number };

const audioConstraints = (deviceId?: string): MediaStreamConstraints => ({
  audio: {
    echoCancellation: false,
    noiseSuppression: false,
    autoGainControl: false,
    ...(deviceId ? { deviceId: { exact: deviceId } } : {}),
  },
  video: false,
});

export default function Tuner({ onClose }: Props) {
  const [listening, setListening] = useState(false);
  const [starting, setStarting] = useState(false);
  const [inputName, setInputName] = useState("GE150Pro USB input");
  const [reading, setReading] = useState<Reading | null>(null);
  const [error, setError] = useState("");
  const streamRef = useRef<MediaStream | null>(null);
  const contextRef = useRef<AudioContext | null>(null);
  const sourceRef = useRef<MediaStreamAudioSourceNode | null>(null);
  const analyserRef = useRef<AnalyserNode | null>(null);
  const bufferRef = useRef<Float32Array | null>(null);
  const frameRef = useRef<number>(0);
  const lastUpdateRef = useRef(0);
  const smoothFrequencyRef = useRef<number | null>(null);
  const mountedRef = useRef(true);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  function stopAudio() {
    cancelAnimationFrame(frameRef.current);
    sourceRef.current?.disconnect();
    analyserRef.current?.disconnect();
    streamRef.current?.getTracks().forEach((track) => track.stop());
    streamRef.current = null;
    sourceRef.current = null;
    analyserRef.current = null;
    bufferRef.current = null;
    smoothFrequencyRef.current = null;
    const context = contextRef.current;
    contextRef.current = null;
    if (context && context.state !== "closed") void context.close();
    setListening(false);
    setReading(null);
  }

  useEffect(() => {
    const escape = (event: KeyboardEvent) => {
      if (event.key === "Escape") onCloseRef.current();
    };
    mountedRef.current = true;
    window.addEventListener("keydown", escape);
    return () => {
      mountedRef.current = false;
      window.removeEventListener("keydown", escape);
      cancelAnimationFrame(frameRef.current);
      sourceRef.current?.disconnect();
      streamRef.current?.getTracks().forEach((track) => track.stop());
      const context = contextRef.current;
      if (context && context.state !== "closed") void context.close();
    };
  }, []);

  async function connect(stream: MediaStream, label: string) {
    const context = new AudioContext();
    try {
      await context.resume();
      if (!mountedRef.current) {
        stream.getTracks().forEach((track) => track.stop());
        await context.close();
        return;
      }
      const source = context.createMediaStreamSource(stream);
      const analyser = context.createAnalyser();
      analyser.fftSize = 8192;
      analyser.smoothingTimeConstant = 0;
      source.connect(analyser);
      streamRef.current = stream;
      contextRef.current = context;
      sourceRef.current = source;
      analyserRef.current = analyser;
      bufferRef.current = new Float32Array(analyser.fftSize);
      setInputName(label);
      setListening(true);
      setError("");

      const update = (time: number) => {
        const activeAnalyser = analyserRef.current;
        const buffer = bufferRef.current;
        if (!activeAnalyser || !buffer) return;
        if (time - lastUpdateRef.current >= 45) {
          activeAnalyser.getFloatTimeDomainData(buffer);
          const detected = detectPitchHz(buffer, context.sampleRate);
          if (detected !== null) {
            const previous = smoothFrequencyRef.current;
            const smoothed = previous !== null && Math.abs(1200 * Math.log2(detected / previous)) < 70
              ? previous * 0.65 + detected * 0.35
              : detected;
            smoothFrequencyRef.current = smoothed;
            const note = noteForFrequency(smoothed);
            setReading({ note: note.name, cents: note.cents, frequency: smoothed });
          } else {
            smoothFrequencyRef.current = null;
            setReading(null);
          }
          lastUpdateRef.current = time;
        }
        frameRef.current = requestAnimationFrame(update);
      };
      frameRef.current = requestAnimationFrame(update);
    } catch (cause) {
      stream.getTracks().forEach((track) => track.stop());
      await context.close().catch(() => undefined);
      throw cause;
    }
  }

  async function startTuner() {
    if (starting || listening) return;
    setStarting(true);
    setError("");
    if (!navigator.mediaDevices?.getUserMedia) {
      setError("This WebView does not provide microphone access. Update macOS and restart the app.");
      setStarting(false);
      return;
    }
    let candidate: MediaStream | null = null;
    try {
      // Request permission first so macOS reveals audio-input device labels.
      candidate = await navigator.mediaDevices.getUserMedia(audioConstraints());
      if (!mountedRef.current) {
        candidate.getTracks().forEach((track) => track.stop());
        return;
      }
      const inputs = (await navigator.mediaDevices.enumerateDevices()).filter((device) => device.kind === "audioinput");
      const pedalInput = inputs.find((device) => /ge\s*150/i.test(device.label));
      if (!pedalInput) {
        throw new Error("GE150Pro is not available as a USB audio input. Reconnect it and check macOS Sound input settings.");
      }
      const currentId = candidate.getAudioTracks()[0]?.getSettings().deviceId;
      if (!mountedRef.current) {
        candidate.getTracks().forEach((track) => track.stop());
        return;
      }
      if (pedalInput.deviceId === currentId) {
        await connect(candidate, pedalInput.label || "GE150Pro USB input");
        candidate = null;
      } else {
        candidate.getTracks().forEach((track) => track.stop());
        candidate = await navigator.mediaDevices.getUserMedia(audioConstraints(pedalInput.deviceId));
        await connect(candidate, pedalInput.label || "GE150Pro USB input");
        candidate = null;
      }
    } catch (cause) {
      candidate?.getTracks().forEach((track) => track.stop());
      if (!mountedRef.current) return;
      const name = cause instanceof DOMException ? cause.name : "";
      setError(name === "NotAllowedError" || name === "SecurityError"
        ? "Microphone access was denied. Allow GE150 Tone Studio in macOS Privacy & Security, then try again."
        : cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (mountedRef.current) setStarting(false);
    }
  }

  const cents = reading ? Math.max(-50, Math.min(50, reading.cents)) : 0;
  const inTune = reading !== null && Math.abs(reading.cents) <= 5;
  const state = !listening ? "READY" : reading === null ? "PLAY A STRING" : inTune ? "IN TUNE" : reading.cents < 0 ? "FLAT" : "SHARP";

  return <div className="tuner-backdrop">
    <section className="tuner-dialog" role="dialog" aria-modal="true" aria-labelledby="tuner-title">
      <header className="tuner-heading"><div><small>GE150 PRO · USB AUDIO</small><h2 id="tuner-title">Chromatic tuner</h2></div><button className="icon-button tuner-close" aria-label="Close tuner" onClick={onClose}>×</button></header>
      <div className={`tuner-state ${inTune ? "in-tune" : ""}`} aria-live="polite">{state}</div>
      <div className="tuner-note" aria-live="polite">{reading?.note ?? "—"}</div>
      <div className="tuner-frequency">{reading ? `${reading.frequency.toFixed(1)} Hz` : "440 Hz reference"}</div>
      <div className="tuner-gauge" aria-label={reading ? `${reading.cents.toFixed(0)} cents` : "No pitch detected"}>
        <div className="tuner-track"><i className="tuner-center"/><i className="tuner-needle" style={{ left: `${50 + cents * 0.7}%` }}/></div>
        <div className="tuner-scale"><span>−50¢</span><span>−25¢</span><span>0</span><span>+25¢</span><span>+50¢</span></div>
      </div>
      <div className="tuner-input"><span>INPUT</span><b>{inputName}</b></div>
      {error && <div className="tuner-error" role="alert">{error}</div>}
      <footer className="tuner-actions">{listening ? <button className="button button-secondary" onClick={stopAudio}>STOP LISTENING</button> : <button className="button button-primary" disabled={starting} onClick={() => void startTuner()}>{starting ? "CONNECTING…" : "ENABLE GE150 AUDIO"}</button>}<small>Audio is analyzed locally and is not recorded.</small></footer>
    </section>
  </div>;
}
