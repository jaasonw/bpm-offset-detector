// Loads the tempo-wasm package from /public/wasm at runtime (fetched, not
// bundled by webpack) and wraps its `analyze` export. Served as static
// files, not resolved through Next's module graph, so we hand-declare the
// shape instead of importing the .d.ts wasm-pack generates alongside it.

export interface TempoCandidate {
  bpm: number;
  offset: number;
  fitness: number;
}

export interface MeterEstimateOut {
  timeSignature: string;
  confidence: number;
  ambiguous: boolean;
  experimental: boolean;
}

export interface AnalyzeResult {
  results: TempoCandidate[];
  // Returned by the wasm module for parity with the CLI, but intentionally
  // not surfaced in the UI — meter estimation accuracy hasn't held up well.
  meterEstimate: MeterEstimateOut | null;
}

export interface AnalyzeOptions {
  minBpm?: number;
  maxBpm?: number;
  subharmonicPreference?: boolean;
}

interface WasmExports {
  default: (wasmUrl: string) => Promise<unknown>;
  analyze: (samples: Float32Array, sampleRate: number, opts: unknown) => AnalyzeResult;
}

let modulePromise: Promise<WasmExports> | null = null;

// A non-literal specifier keeps tsc from trying (and failing) to resolve
// this as a module on disk — it's a runtime-fetched static asset, not part
// of the bundle graph.
const wasmGlueUrl = "/wasm/tempo_wasm.js";

function loadWasm(): Promise<WasmExports> {
  if (!modulePromise) {
    modulePromise = (async () => {
      const mod = (await import(/* webpackIgnore: true */ wasmGlueUrl)) as WasmExports;
      await mod.default("/wasm/tempo_wasm_bg.wasm");
      return mod;
    })();
  }
  return modulePromise;
}

export async function analyzeAudio(
  samples: Float32Array,
  sampleRate: number,
  opts: AnalyzeOptions = {},
): Promise<AnalyzeResult> {
  const mod = await loadWasm();
  return mod.analyze(samples, sampleRate, opts);
}
