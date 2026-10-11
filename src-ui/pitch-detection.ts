export function detectPitchHz(samples: ArrayLike<number>, sampleRate: number): number | null {
  const size = Math.floor(samples.length / 2);
  const rate = sampleRate / 2;
  if (size < 1024 || !Number.isFinite(rate) || rate <= 0) return null;

  const signal = new Float32Array(size);
  let mean = 0;
  for (let i = 0; i < size; i++) mean += samples[i * 2];
  mean /= size;
  let power = 0;
  for (let i = 0; i < size; i++) {
    const value = samples[i * 2] - mean;
    signal[i] = value;
    power += value * value;
  }
  if (Math.sqrt(power / size) < 0.003) return null;

  const minLag = Math.max(2, Math.floor(rate / 1200));
  const maxLag = Math.min(Math.floor(rate / 45), Math.floor(size / 2));
  const difference = new Float64Array(maxLag + 1);
  const normalized = new Float64Array(maxLag + 1);
  const window = size - maxLag;
  for (let lag = 1; lag <= maxLag; lag++) {
    let sum = 0;
    for (let i = 0; i < window; i++) {
      const delta = signal[i] - signal[i + lag];
      sum += delta * delta;
    }
    difference[lag] = sum;
  }

  let cumulative = 0;
  for (let lag = 1; lag <= maxLag; lag++) {
    cumulative += difference[lag];
    normalized[lag] = cumulative > 0 ? difference[lag] * lag / cumulative : 1;
  }

  let bestLag = -1;
  let bestScore = 1;
  for (let lag = minLag; lag <= maxLag; lag++) {
    if (normalized[lag] < bestScore) {
      bestLag = lag;
      bestScore = normalized[lag];
    }
    if (normalized[lag] < 0.15) {
      while (lag < maxLag && normalized[lag + 1] < normalized[lag]) lag++;
      bestLag = lag;
      bestScore = normalized[lag];
      break;
    }
  }
  if (bestLag < 0 || bestScore > 0.35) return null;

  const left = normalized[Math.max(minLag, bestLag - 1)];
  const center = normalized[bestLag];
  const right = normalized[Math.min(maxLag, bestLag + 1)];
  const denominator = 2 * (left - 2 * center + right);
  const shift = denominator === 0 ? 0 : Math.max(-1, Math.min(1, (left - right) / denominator));
  const frequency = rate / (bestLag + shift);
  return Number.isFinite(frequency) && frequency >= 45 && frequency <= 1200 ? frequency : null;
}

const NOTES = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];

export function noteForFrequency(frequency: number, reference = 440) {
  const midi = Math.round(69 + 12 * Math.log2(frequency / reference));
  const target = reference * 2 ** ((midi - 69) / 12);
  return {
    name: `${NOTES[(midi % 12 + 12) % 12]}${Math.floor(midi / 12) - 1}`,
    target,
    cents: 1200 * Math.log2(frequency / target),
  };
}
