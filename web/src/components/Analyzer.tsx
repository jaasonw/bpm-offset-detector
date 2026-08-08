"use client";

import { useCallback, useRef, useState, type DragEvent } from "react";
import { decodeToMono } from "@/lib/decode-audio";
import { analyzeAudio, type AnalyzeResult } from "@/lib/tempo-wasm";

function formatOffset(seconds: number): string {
  const sign = seconds < 0 ? "-" : "";
  const abs = Math.abs(seconds);
  const m = Math.floor(abs / 60);
  const s = (abs % 60).toFixed(3).padStart(6, "0");
  return `${sign}${m}:${s}`;
}

export default function Analyzer() {
  const [minBpm, setMinBpm] = useState(40);
  const [maxBpm, setMaxBpm] = useState(260);
  const [subharmonicPreference, setSubharmonicPreference] = useState(true);
  const [start, setStart] = useState(0);
  const [duration, setDuration] = useState(60);
  const [status, setStatus] = useState<"idle" | "decoding" | "analyzing" | "done" | "error">(
    "idle",
  );
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<AnalyzeResult | null>(null);
  const [fileName, setFileName] = useState<string | null>(null);
  const [dragActive, setDragActive] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const runAnalysis = useCallback(
    async (file: File) => {
      setFileName(file.name);
      setError(null);
      setResult(null);
      try {
        setStatus("decoding");
        const { samples, sampleRate } = await decodeToMono(file, start, duration);
        if (samples.length === 0) {
          throw new Error("Decoded to zero samples — check the start/duration range.");
        }
        setStatus("analyzing");
        const out = await analyzeAudio(samples, sampleRate, {
          minBpm,
          maxBpm,
          subharmonicPreference,
        });
        setResult(out);
        setStatus("done");
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        setStatus("error");
      }
    },
    [start, duration, minBpm, maxBpm, subharmonicPreference],
  );

  const busy = status === "decoding" || status === "analyzing";

  const handleDrop = useCallback(
    (e: DragEvent<HTMLDivElement>) => {
      e.preventDefault();
      setDragActive(false);
      const file = e.dataTransfer.files?.[0];
      if (file) void runAnalysis(file);
    },
    [runAnalysis],
  );

  return (
    <div className="flex w-full max-w-2xl flex-col gap-6">
      <div
        onDragOver={(e) => {
          e.preventDefault();
          setDragActive(true);
        }}
        onDragLeave={() => setDragActive(false)}
        onDrop={handleDrop}
        onClick={() => fileInputRef.current?.click()}
        role="button"
        tabIndex={0}
        className={`flex cursor-pointer flex-col items-center justify-center gap-2 rounded-xl border-2 border-dashed p-12 text-center transition-colors ${
          dragActive
            ? "border-zinc-900 bg-zinc-100 dark:border-zinc-100 dark:bg-zinc-900"
            : "border-zinc-300 hover:border-zinc-400 dark:border-zinc-700 dark:hover:border-zinc-600"
        }`}
      >
        <input
          ref={fileInputRef}
          type="file"
          accept="audio/*,.mp3,.wav,.flac,.ogg,.m4a,.aac"
          className="hidden"
          onChange={(e) => {
            const file = e.target.files?.[0];
            if (file) void runAnalysis(file);
            e.target.value = "";
          }}
        />
        {busy ? (
          <p className="text-sm text-zinc-600 dark:text-zinc-400">
            {status === "decoding" ? "Decoding..." : "Analyzing..."} {fileName}
          </p>
        ) : (
          <>
            <p className="text-sm font-medium text-zinc-700 dark:text-zinc-300">
              Drop an audio file here, or click to choose one
            </p>
            <p className="text-xs text-zinc-500">mp3, wav, flac, ogg, m4a, aac</p>
          </>
        )}
      </div>

      <details className="rounded border border-zinc-200 p-3 text-sm dark:border-zinc-800">
        <summary className="cursor-pointer font-medium text-zinc-700 dark:text-zinc-300">
          Options
        </summary>
        <div className="mt-3 grid grid-cols-2 gap-3">
          <label className="flex flex-col gap-1">
            <span className="text-xs text-zinc-500">Min BPM</span>
            <input
              type="number"
              value={minBpm}
              onChange={(e) => setMinBpm(Number(e.target.value))}
              className="rounded border border-zinc-300 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900"
            />
          </label>
          <label className="flex flex-col gap-1">
            <span className="text-xs text-zinc-500">Max BPM</span>
            <input
              type="number"
              value={maxBpm}
              onChange={(e) => setMaxBpm(Number(e.target.value))}
              className="rounded border border-zinc-300 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900"
            />
          </label>
          <label className="flex flex-col gap-1">
            <span className="text-xs text-zinc-500">Start (sec)</span>
            <input
              type="number"
              value={start}
              onChange={(e) => setStart(Number(e.target.value))}
              className="rounded border border-zinc-300 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900"
            />
          </label>
          <label className="flex flex-col gap-1">
            <span className="text-xs text-zinc-500">Duration (sec)</span>
            <input
              type="number"
              value={duration}
              onChange={(e) => setDuration(Number(e.target.value))}
              className="rounded border border-zinc-300 px-2 py-1 dark:border-zinc-700 dark:bg-zinc-900"
            />
          </label>
          <label className="col-span-2 flex items-center gap-2">
            <input
              type="checkbox"
              checked={subharmonicPreference}
              onChange={(e) => setSubharmonicPreference(e.target.checked)}
            />
            <span className="text-xs text-zinc-500">
              Subharmonic preference (fixes triplet-feel songs reported at 3x tempo)
            </span>
          </label>
        </div>
      </details>

      {error && (
        <p className="rounded border border-red-300 bg-red-50 p-3 text-sm text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200">
          {error}
        </p>
      )}

      {result && (
        <div className="flex flex-col gap-4">
          <table className="w-full border-collapse text-sm">
            <thead>
              <tr className="border-b border-zinc-300 text-left dark:border-zinc-700">
                <th className="py-1 pr-4 font-medium">BPM</th>
                <th className="py-1 pr-4 font-medium">Offset</th>
                <th className="py-1 font-medium">Fitness</th>
              </tr>
            </thead>
            <tbody>
              {result.results.map((r, i) => (
                <tr
                  key={i}
                  className={
                    i === 0
                      ? "font-semibold text-zinc-900 dark:text-zinc-50"
                      : "text-zinc-600 dark:text-zinc-400"
                  }
                >
                  <td className="py-1 pr-4">{r.bpm.toFixed(2)}</td>
                  <td className="py-1 pr-4">{formatOffset(r.offset)}</td>
                  <td className="py-1">{r.fitness.toFixed(3)}</td>
                </tr>
              ))}
            </tbody>
          </table>

          <details className="text-xs text-zinc-500">
            <summary className="cursor-pointer">Known limitations</summary>
            <ul className="mt-2 list-disc pl-4">
              <li>
                Offset tends to run late vs. human-marked beats — roughly +2ms on sharp
                transients up to +27ms on soft ones.
              </li>
              <li>
                When the strongest transient layer sits on the offbeat, the estimator can lock
                onto the offbeat grid instead (half a beat off).
              </li>
              <li>
                Browser decoding doesn&apos;t trim MP3 encoder-delay padding the way the CLI
                does, so offsets on some MP3s may run a few tens of ms later than the CLI&apos;s.
              </li>
            </ul>
          </details>
        </div>
      )}
    </div>
  );
}
