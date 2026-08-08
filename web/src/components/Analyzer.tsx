"use client";

import { useCallback, useEffect, useId, useRef, useState, type DragEvent } from "react";
import { decodeToMono } from "@/lib/decode-audio";
import { analyzeAudio, type AnalyzeResult } from "@/lib/tempo-wasm";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

function formatOffset(seconds: number): string {
  return `${Math.round(seconds * 1000)}ms`;
}

const OPTIONS_STORAGE_KEY = "tempo-detector-options";

interface SavedOptions {
  minBpm: number;
  maxBpm: number;
  subharmonicPreference: boolean;
  start: number;
  duration: number;
}

function loadSavedOptions(): Partial<SavedOptions> {
  try {
    const raw = localStorage.getItem(OPTIONS_STORAGE_KEY);
    return raw ? (JSON.parse(raw) as Partial<SavedOptions>) : {};
  } catch {
    return {};
  }
}

// Persists imperatively from each onChange rather than reactively off state
// (a `useEffect` keyed on the option values would also fire once on mount,
// racing the mount-time restore below and clobbering it with stale
// defaults — reliably, since React 18 Strict Mode double-invokes effects
// in dev).
function persistOption<K extends keyof SavedOptions>(key: K, value: SavedOptions[K]) {
  try {
    localStorage.setItem(OPTIONS_STORAGE_KEY, JSON.stringify({ ...loadSavedOptions(), [key]: value }));
  } catch {
    // Storage unavailable (private browsing, quota, etc.) — not persisted.
  }
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
  const [currentFile, setCurrentFile] = useState<File | null>(null);
  const [dragActive, setDragActive] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const fieldId = useId();

  // Restore saved options on mount. Runs once, after hydration, so there's
  // no server/client mismatch to worry about (defaults above always match
  // the server-rendered markup).
  useEffect(() => {
    const saved = loadSavedOptions();
    // eslint-disable-next-line react-hooks/set-state-in-effect
    if (typeof saved.minBpm === "number") setMinBpm(saved.minBpm);
    if (typeof saved.maxBpm === "number") setMaxBpm(saved.maxBpm);
    if (typeof saved.subharmonicPreference === "boolean")
      setSubharmonicPreference(saved.subharmonicPreference);
    if (typeof saved.start === "number") setStart(saved.start);
    if (typeof saved.duration === "number") setDuration(saved.duration);
  }, []);

  const runAnalysis = useCallback(
    async (file: File) => {
      setCurrentFile(file);
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

  const handleReanalyze = useCallback(() => {
    if (currentFile) void runAnalysis(currentFile);
  }, [currentFile, runAnalysis]);

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
    <div className="flex w-full max-w-2xl flex-col gap-3">
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
        className={`flex cursor-pointer flex-col items-center justify-center gap-1.5 rounded-xl border-2 border-dashed p-6 text-center transition-colors ${
          dragActive ? "border-foreground bg-muted" : "border-border hover:border-foreground/40"
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
          <p className="text-sm text-muted-foreground">
            {status === "decoding" ? "Decoding..." : "Analyzing..."} {currentFile?.name}
          </p>
        ) : (
          <>
            <p className="text-sm font-medium text-foreground">
              Drop an audio file here, or click to choose one
            </p>
            <p className="text-xs text-muted-foreground">mp3, wav, flac, ogg, m4a, aac</p>
          </>
        )}
      </div>

      {currentFile && !busy && (
        <div className="flex items-center justify-between gap-2 rounded-lg border border-border px-3 py-2 text-xs text-muted-foreground">
          <span className="truncate">{currentFile.name}</span>
          <Button
            type="button"
            variant="outline"
            size="xs"
            onClick={(e) => {
              e.stopPropagation();
              handleReanalyze();
            }}
          >
            Reanalyze
          </Button>
        </div>
      )}

      <details className="rounded-lg border border-border p-2.5 text-sm">
        <summary className="cursor-pointer font-medium text-foreground">Options</summary>
        <div className="mt-2 grid grid-cols-2 gap-1.5">
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${fieldId}-min-bpm`} className="text-xs text-muted-foreground">
              Min BPM
            </Label>
            <Input
              id={`${fieldId}-min-bpm`}
              type="number"
              value={minBpm}
              onChange={(e) => {
                const v = Number(e.target.value);
                setMinBpm(v);
                persistOption("minBpm", v);
              }}
            />
          </div>
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${fieldId}-max-bpm`} className="text-xs text-muted-foreground">
              Max BPM
            </Label>
            <Input
              id={`${fieldId}-max-bpm`}
              type="number"
              value={maxBpm}
              onChange={(e) => {
                const v = Number(e.target.value);
                setMaxBpm(v);
                persistOption("maxBpm", v);
              }}
            />
          </div>
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${fieldId}-start`} className="text-xs text-muted-foreground">
              Start (sec)
            </Label>
            <Input
              id={`${fieldId}-start`}
              type="number"
              value={start}
              onChange={(e) => {
                const v = Number(e.target.value);
                setStart(v);
                persistOption("start", v);
              }}
            />
          </div>
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${fieldId}-duration`} className="text-xs text-muted-foreground">
              Duration (sec)
            </Label>
            <Input
              id={`${fieldId}-duration`}
              type="number"
              value={duration}
              onChange={(e) => {
                const v = Number(e.target.value);
                setDuration(v);
                persistOption("duration", v);
              }}
            />
          </div>
          <div className="col-span-2 flex items-center gap-2">
            <Checkbox
              id={`${fieldId}-subharmonic`}
              checked={subharmonicPreference}
              onCheckedChange={(checked) => {
                const v = checked === true;
                setSubharmonicPreference(v);
                persistOption("subharmonicPreference", v);
              }}
            />
            <Label
              htmlFor={`${fieldId}-subharmonic`}
              className="text-xs font-normal text-muted-foreground"
            >
              Subharmonic preference (fixes triplet-feel songs reported at 3x tempo)
            </Label>
          </div>
        </div>
      </details>

      {error && (
        <p className="rounded-lg border border-destructive/30 bg-destructive/10 p-3 text-sm text-destructive">
          {error}
        </p>
      )}

      {result && (
        <div className="flex flex-col gap-2">
          <table className="w-full border-collapse text-sm">
            <thead>
              <tr className="border-b border-border text-left">
                <th className="py-1 pr-4 font-medium">BPM</th>
                <th className="py-1 pr-4 font-medium">Offset</th>
                <th className="py-1 font-medium">Fitness</th>
              </tr>
            </thead>
            <tbody>
              {result.results.map((r, i) => (
                <tr
                  key={i}
                  className={i === 0 ? "font-semibold text-foreground" : "text-muted-foreground"}
                >
                  <td className="py-1 pr-4">{r.bpm.toFixed(2)}</td>
                  <td className="py-1 pr-4">{formatOffset(r.offset)}</td>
                  <td className="py-1">{r.fitness.toFixed(3)}</td>
                </tr>
              ))}
            </tbody>
          </table>

          <details className="text-xs text-muted-foreground">
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
