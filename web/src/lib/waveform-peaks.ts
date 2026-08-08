// Reduces PCM to per-pixel min/max pairs for waveform drawing. A 60s 44.1kHz
// mono track is 2.6M samples, so this runs once per (range, width) pair and is
// memoized by the caller — never per animation frame.

export interface Peaks {
  min: Float32Array;
  max: Float32Array;
}

/**
 * Min/max reduction of `samples[from..to)` into `buckets` columns.
 *
 * When the range is shorter than `buckets` (a zoomed-in detail view), buckets
 * map back to the same sample and the result is effectively a step-sampled
 * waveform, which is the correct rendering at that zoom.
 */
export function computePeaks(
  samples: Float32Array,
  buckets: number,
  from = 0,
  to = samples.length,
): Peaks {
  const width = Math.max(1, Math.floor(buckets));
  const min = new Float32Array(width);
  const max = new Float32Array(width);

  const start = Math.max(0, Math.min(from, samples.length));
  const end = Math.max(start, Math.min(to, samples.length));
  const span = end - start;
  if (span === 0) return { min, max };

  for (let b = 0; b < width; b++) {
    const lo = start + Math.floor((b * span) / width);
    const hi = Math.max(lo + 1, start + Math.floor(((b + 1) * span) / width));
    let lowest = Infinity;
    let highest = -Infinity;
    for (let i = lo; i < hi; i++) {
      const v = samples[i];
      if (v < lowest) lowest = v;
      if (v > highest) highest = v;
    }
    min[b] = lowest;
    max[b] = highest;
  }

  return { min, max };
}
