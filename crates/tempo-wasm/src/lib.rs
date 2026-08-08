//! wasm-bindgen bindings for `tempo-core`, exposing a single `analyze`
//! function for browser use. Callers decode audio with the Web Audio API's
//! `decodeAudioData`, downmix to mono `f32`, and pass the PCM straight in —
//! mirrors `tempo-cli`'s `ResultOut`/`MeterOut` JSON shape so the web UI and
//! CLI `--json` output stay in sync. (The web UI doesn't currently surface
//! meter estimation — accuracy hasn't held up well enough — but it's kept
//! here in the response shape for parity with the CLI and future use.)

use serde::{Deserialize, Serialize};
use tempo_core::{detect_with_onsets, estimate_meter, DetectOptions};
use wasm_bindgen::prelude::*;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AnalyzeOptions {
    pub min_bpm: f64,
    pub max_bpm: f64,
    pub subharmonic_preference: bool,
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        let d = DetectOptions::default();
        AnalyzeOptions {
            min_bpm: d.min_bpm,
            max_bpm: d.max_bpm,
            subharmonic_preference: d.subharmonic_preference,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ResultOut {
    bpm: f64,
    offset: f64,
    fitness: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MeterOut {
    time_signature: String,
    confidence: f64,
    ambiguous: bool,
    /// Always true: meter estimation is a heuristic hint, not ground truth.
    experimental: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AnalyzeOut {
    results: Vec<ResultOut>,
    meter_estimate: Option<MeterOut>,
}

// Named to avoid colliding with wasm-bindgen's conventional default-export
// import alias `init` (`import init, { analyze } from './tempo_wasm.js'`).
#[wasm_bindgen(start)]
pub fn init_panic_hook() {
    console_error_panic_hook::set_once();
}

/// Runs full tempo/offset/meter detection on mono `f32` PCM and returns a
/// JSON-shaped result (`{ results: [...], meterEstimate: {...} | null }`).
/// `opts` is an optional JS object with `minBpm`/`maxBpm`/`subharmonicPreference`
/// (all optional; defaults match the CLI).
#[wasm_bindgen]
pub fn analyze(samples: &[f32], sample_rate: u32, opts: JsValue) -> Result<JsValue, JsValue> {
    let opts: AnalyzeOptions = if opts.is_undefined() || opts.is_null() {
        AnalyzeOptions::default()
    } else {
        serde_wasm_bindgen::from_value(opts).map_err(|e| JsValue::from_str(&e.to_string()))?
    };

    let detect_opts = DetectOptions {
        min_bpm: opts.min_bpm,
        max_bpm: opts.max_bpm,
        subharmonic_preference: opts.subharmonic_preference,
    };

    let (onsets, results, context) = detect_with_onsets(samples, sample_rate, &detect_opts);

    let meter = results.first().and_then(|top| {
        estimate_meter(&onsets, samples, sample_rate, top.bpm, top.offset, &context)
    });

    let out = AnalyzeOut {
        results: results
            .into_iter()
            .map(|r| ResultOut {
                bpm: r.bpm,
                offset: r.offset,
                fitness: r.fitness,
            })
            .collect(),
        meter_estimate: meter.map(|m| MeterOut {
            time_signature: m.notation,
            confidence: m.confidence,
            ambiguous: m.ambiguous,
            experimental: true,
        }),
    };

    serde_wasm_bindgen::to_value(&out).map_err(|e| JsValue::from_str(&e.to_string()))
}
