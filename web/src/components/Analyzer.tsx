"use client";

import { useCallback, useEffect, useId, useRef, useState, type DragEvent } from "react";
import { decodeToMono, type DecodedAudio } from "@/lib/decode-audio";
import { analyzeAudio, type AnalyzeResult } from "@/lib/tempo-wasm";
import WaveformView from "@/components/WaveformView";
import { loadSavedOptions, persistOption } from "@/lib/options-storage";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

// Note the last sentence: the correction passes in bpm.rs promote a candidate
// by position without rewriting its fitness, so the top row legitimately shows
// a lower number than the row beneath it. Without saying so, the table reads
// like a sorting bug.
const FITNESS_HELP =
  "How sharply onsets line up on this tempo's grid — higher is a cleaner fit. " +
  "The scale is arbitrary, so only compare rows within one song, not between songs. " +
  "The top candidate can show a lower fitness when a correction pass overrode the raw score.";

function formatOffset(seconds: number): string {
  return `${Math.round(seconds * 1000)}ms`;
}

export default function Analyzer() {
  const [minBpm, setMinBpm] = useState(40);
  const [maxBpm, setMaxBpm] = useState(260);
  const [subharmonicPreference, setSubharmonicPreference] = useState(true);
  const [start, setStart] = useState(0);
  const [duration, setDuration] = useState(60);
  // Mirrors the committed numbers above as free-typed text: a controlled
  // number input that snaps an emptied field back to "0" makes it impossible
  // to clear and retype, so these stay uncommitted strings until blur/submit.
  const [minBpmInput, setMinBpmInput] = useState(() => String(minBpm));
  const [maxBpmInput, setMaxBpmInput] = useState(() => String(maxBpm));
  const [startInput, setStartInput] = useState(() => String(start));
  const [durationInput, setDurationInput] = useState(() => String(duration));
  const [status, setStatus] = useState<"idle" | "decoding" | "analyzing" | "done" | "error">(
    "idle",
  );
  const [error, setError] = useState<string | null>(null);
  const [result, setResult] = useState<AnalyzeResult | null>(null);
  // Kept so the waveform view draws exactly the PCM that was analyzed — the
  // returned offsets are relative to this slice, not to the source file.
  const [decoded, setDecoded] = useState<DecodedAudio | null>(null);
  const [selectedIndex, setSelectedIndex] = useState(0);
  // Bumped per completed analysis and used as WaveformView's key, so a new
  // file gets a fresh player and transport rather than an in-place reset.
  const [runId, setRunId] = useState(0);
  const [currentFile, setCurrentFile] = useState<File | null>(null);
  const [dragActive, setDragActive] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const fieldId = useId();

  // Restore saved options on mount. Runs once, after hydration, so there's
  // no server/client mismatch to worry about (defaults above always match
  // the server-rendered markup).
  useEffect(() => {
    const saved = loadSavedOptions();
    /* eslint-disable react-hooks/set-state-in-effect */
    if (typeof saved.minBpm === "number") {
      setMinBpm(saved.minBpm);
      setMinBpmInput(String(saved.minBpm));
    }
    if (typeof saved.maxBpm === "number") {
      setMaxBpm(saved.maxBpm);
      setMaxBpmInput(String(saved.maxBpm));
    }
    if (typeof saved.subharmonicPreference === "boolean")
      setSubharmonicPreference(saved.subharmonicPreference);
    if (typeof saved.start === "number") {
      setStart(saved.start);
      setStartInput(String(saved.start));
    }
    if (typeof saved.duration === "number") {
      setDuration(saved.duration);
      setDurationInput(String(saved.duration));
    }
    /* eslint-enable react-hooks/set-state-in-effect */
  }, []);

  /** Parses the free-typed option fields, falling back to the last committed
   *  number for anything unparseable (e.g. emptied mid-edit), and pushes both
   *  the committed state and its text mirror back in sync. Returns the parsed
   *  values directly so a caller doesn't have to wait a render for state to
   *  catch up before starting an analysis with them. */
  const commitOptions = useCallback(() => {
    const commit = (
      raw: string,
      current: number,
      setNum: (v: number) => void,
      setStr: (v: string) => void,
      key: "minBpm" | "maxBpm" | "start" | "duration",
    ) => {
      const parsed = parseFloat(raw);
      const next = Number.isFinite(parsed) ? parsed : current;
      setNum(next);
      setStr(String(next));
      persistOption(key, next);
      return next;
    };
    return {
      minBpm: commit(minBpmInput, minBpm, setMinBpm, setMinBpmInput, "minBpm"),
      maxBpm: commit(maxBpmInput, maxBpm, setMaxBpm, setMaxBpmInput, "maxBpm"),
      start: commit(startInput, start, setStart, setStartInput, "start"),
      duration: commit(durationInput, duration, setDuration, setDurationInput, "duration"),
    };
  }, [minBpmInput, maxBpmInput, startInput, durationInput, minBpm, maxBpm, start, duration]);

  const runAnalysis = useCallback(
    async (file: File, opts?: { minBpm: number; maxBpm: number; start: number; duration: number }) => {
      const { minBpm: mn, maxBpm: mx, start: st, duration: du } = opts ?? {
        minBpm,
        maxBpm,
        start,
        duration,
      };
      setCurrentFile(file);
      setError(null);
      setResult(null);
      setDecoded(null);
      setSelectedIndex(0);
      try {
        setStatus("decoding");
        const decodedAudio = await decodeToMono(file, st, du);
        const { samples, sampleRate } = decodedAudio;
        if (samples.length === 0) {
          throw new Error("Decoded to zero samples — check the start/duration range.");
        }
        setStatus("analyzing");
        const out = await analyzeAudio(samples, sampleRate, {
          minBpm: mn,
          maxBpm: mx,
          subharmonicPreference,
        });
        setResult(out);
        setDecoded(decodedAudio);
        setRunId((n) => n + 1);
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
    const opts = commitOptions();
    if (currentFile) void runAnalysis(currentFile, opts);
  }, [currentFile, runAnalysis, commitOptions]);

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
        <form
          className="mt-2 grid grid-cols-2 gap-1.5"
          onSubmit={(e) => {
            e.preventDefault();
            handleReanalyze();
          }}
        >
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${fieldId}-min-bpm`} className="text-xs text-muted-foreground">
              Min BPM
            </Label>
            <Input
              id={`${fieldId}-min-bpm`}
              type="number"
              inputMode="decimal"
              value={minBpmInput}
              onChange={(e) => setMinBpmInput(e.target.value)}
              onBlur={commitOptions}
            />
          </div>
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${fieldId}-max-bpm`} className="text-xs text-muted-foreground">
              Max BPM
            </Label>
            <Input
              id={`${fieldId}-max-bpm`}
              type="number"
              inputMode="decimal"
              value={maxBpmInput}
              onChange={(e) => setMaxBpmInput(e.target.value)}
              onBlur={commitOptions}
            />
          </div>
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${fieldId}-start`} className="text-xs text-muted-foreground">
              Start (sec)
            </Label>
            <Input
              id={`${fieldId}-start`}
              type="number"
              inputMode="decimal"
              value={startInput}
              onChange={(e) => setStartInput(e.target.value)}
              onBlur={commitOptions}
            />
          </div>
          <div className="flex flex-col gap-1">
            <Label htmlFor={`${fieldId}-duration`} className="text-xs text-muted-foreground">
              Duration (sec)
            </Label>
            <Input
              id={`${fieldId}-duration`}
              type="number"
              inputMode="decimal"
              value={durationInput}
              onChange={(e) => setDurationInput(e.target.value)}
              onBlur={commitOptions}
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
          {/* Invisible submit target: without a submit control, Enter in a
              multi-field form doesn't reliably fire onSubmit in every browser. */}
          <button type="submit" className="hidden" aria-hidden="true" tabIndex={-1} />
        </form>
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
                <th className="py-1 font-medium">
                  Fitness
                  <span
                    // Native title rather than a tooltip component: this is the
                    // only tooltip in the app, and it works on touch-and-hold
                    // and with a screen reader without pulling in a primitive.
                    tabIndex={0}
                    title={FITNESS_HELP}
                    aria-label={FITNESS_HELP}
                    className="ml-1 cursor-help rounded-full border border-border px-1 text-[10px] font-normal text-muted-foreground"
                  >
                    ?
                  </span>
                </th>
              </tr>
            </thead>
            <tbody>
              {result.results.map((r, i) => (
                <tr
                  key={i}
                  aria-selected={i === selectedIndex}
                  tabIndex={0}
                  onClick={() => setSelectedIndex(i)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" || e.key === " ") {
                      e.preventDefault();
                      setSelectedIndex(i);
                    }
                  }}
                  className={`cursor-pointer outline-none ${
                    i === selectedIndex
                      ? "bg-accent font-semibold text-foreground"
                      : "text-muted-foreground hover:bg-accent/50"
                  }`}
                >
                  <td className="py-1 pr-4">
                    {r.bpm.toFixed(2)}
                    {/* Ranking is a separate signal from selection — the top
                        candidate stays marked even while inspecting another. */}
                    {i === 0 && <span className="ml-1.5 text-xs font-normal">(top)</span>}
                  </td>
                  <td className="py-1 pr-4">{formatOffset(r.offset)}</td>
                  <td className="py-1">{r.fitness.toFixed(3)}</td>
                </tr>
              ))}
            </tbody>
          </table>

          {decoded && result.results[selectedIndex] && (
            <WaveformView
              key={runId}
              samples={decoded.samples}
              sampleRate={decoded.sampleRate}
              startSeconds={start}
              bpm={result.results[selectedIndex].bpm}
              offset={result.results[selectedIndex].offset}
            />
          )}

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
