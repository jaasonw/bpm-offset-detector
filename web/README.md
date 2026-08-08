# BPM/offset detector — web UI

Next.js frontend for the `tempo-core` algorithm. Upload an audio file,
get BPM/offset/meter candidates back. Analysis runs entirely client-side —
audio is decoded with the browser's Web Audio API and analyzed by
`tempo-core` compiled to WebAssembly (`crates/tempo-wasm`). Nothing is
uploaded to a server.

## Development

```sh
npm install
npm run wasm:build   # rebuilds crates/tempo-wasm and copies pkg output into public/wasm/
npm run dev
```

`public/wasm/tempo_wasm.js` + `tempo_wasm_bg.wasm` are **committed build
artifacts**, not generated at deploy time — Vercel's build image has no
Rust toolchain. Whenever `tempo-core` or `tempo-wasm` changes:

```sh
npm run wasm:build
git add public/wasm
git commit
```

then `npm run build` / deploy as normal.

## Architecture

- `src/lib/decode-audio.ts` — decodes the uploaded file via
  `AudioContext.decodeAudioData` and downmixes to mono `f32`, mirroring
  `tempo-cli`'s `decode.rs`. Note: browser decoding doesn't do the CLI's
  gapless MP3 encoder-delay trim, so offsets on some MP3s may run a few
  tens of ms later than the CLI's.
- `src/lib/tempo-wasm.ts` — fetches `public/wasm/tempo_wasm.js` at runtime
  (not bundled by webpack/Turbopack — it's a plain static asset) and calls
  its `analyze()` export.
- `src/components/Analyzer.tsx` — upload UI, options (min/max BPM,
  subharmonic preference, start/duration trim), results table.

## Deploy on Vercel

Set the project's root directory to `web`. No environment variables
or serverless functions needed — this is a fully static site.
