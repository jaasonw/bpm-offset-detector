// One localStorage blob for every user-facing setting, so preferences survive
// both a new upload (the waveform view remounts per analysis) and a reload.

const STORAGE_KEY = "tempo-detector-options";

export interface SavedOptions {
  minBpm: number;
  maxBpm: number;
  subharmonicPreference: boolean;
  start: number;
  duration: number;
  volume: number;
}

export function loadSavedOptions(): Partial<SavedOptions> {
  if (typeof window === "undefined") return {};
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw ? (JSON.parse(raw) as Partial<SavedOptions>) : {};
  } catch {
    return {};
  }
}

// Persists imperatively from each onChange rather than reactively off state
// (a `useEffect` keyed on the option values would also fire once on mount,
// racing the mount-time restore in Analyzer and clobbering it with stale
// defaults — reliably, since React 18 Strict Mode double-invokes effects
// in dev).
export function persistOption<K extends keyof SavedOptions>(key: K, value: SavedOptions[K]) {
  if (typeof window === "undefined") return;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify({ ...loadSavedOptions(), [key]: value }));
  } catch {
    // Storage unavailable (private browsing, quota, etc.) — not persisted.
  }
}

/** Reads a saved value, falling back when absent or of the wrong type. */
export function savedOr<K extends keyof SavedOptions>(
  key: K,
  fallback: SavedOptions[K],
): SavedOptions[K] {
  const value = loadSavedOptions()[key];
  return typeof value === typeof fallback ? (value as SavedOptions[K]) : fallback;
}
