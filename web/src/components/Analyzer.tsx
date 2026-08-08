"use client";

import { useCallback, useId, useRef, useState, type DragEvent } from "react";
import { decodeToMono } from "@/lib/decode-audio";
import { analyzeAudio, type AnalyzeResult } from "@/lib/tempo-wasm";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

function formatOffset(seconds: number): string {
  return `${Math.round(seconds * 1000)}ms`;
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
  const fieldId = useId();

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
            {status === "decoding" ? "Decoding..." : "Analyzing..."} {fileName}
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
              onChange={(e) => setMinBpm(Number(e.target.value))}
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
              onChange={(e) => setMaxBpm(Number(e.target.value))}
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
              onChange={(e) => setStart(Number(e.target.value))}
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
              onChange={(e) => setDuration(Number(e.target.value))}
            />
          </div>
          <div className="col-span-2 flex items-center gap-2">
            <Checkbox
              id={`${fieldId}-subharmonic`}
              checked={subharmonicPreference}
              onCheckedChange={(checked) => setSubharmonicPreference(checked === true)}
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
