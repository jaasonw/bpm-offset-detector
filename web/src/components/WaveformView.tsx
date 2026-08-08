"use client";

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTheme } from "next-themes";
import { computePeaks, type Peaks } from "@/lib/waveform-peaks";
import { BeatPlayer } from "@/lib/beat-player";
import { persistOption, savedOr } from "@/lib/options-storage";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";

// The overview is for navigation only, so it stays thin; the detail pane is
// where alignment is actually judged and gets the vertical room.
const OVERVIEW_HEIGHT = 56;
const DETAIL_HEIGHT = 184;
/** Detail window span. 2s across ~700px is ~3ms/px — fine enough to see the
 *  documented offset bias (+2ms..+27ms) as an actual gap. */
const DETAIL_SPAN_S = 2;
/** Below this spacing the overview grid is a solid wall of ink, not a grid. */
const MIN_GRID_SPACING_PX = 3;

// The app palette is fully achromatic, so the beat grid gets the one chromatic
// accent — it has to read as "not part of the waveform" at a glance.
const GRID_COLOR = "#f59e0b";
const PLAYHEAD_COLOR = "#ef4444";

interface Props {
  samples: Float32Array;
  sampleRate: number;
  /** Offset of the analyzed slice within the source file, for display only. */
  startSeconds: number;
  bpm: number;
  /** Seconds from the start of the analyzed slice. */
  offset: number;
}

interface Palette {
  wave: string;
  axis: string;
  window: string;
}

function readPalette(el: HTMLElement): Palette {
  const cs = getComputedStyle(el);
  const v = (name: string, fallback: string) => cs.getPropertyValue(name).trim() || fallback;
  return {
    wave: v("--muted-foreground", "#888"),
    axis: v("--border", "#ccc"),
    window: v("--accent", "#eee"),
  };
}

function formatTime(seconds: number): string {
  const s = Math.max(0, seconds);
  const mins = Math.floor(s / 60);
  const secs = s - mins * 60;
  return `${mins}:${secs.toFixed(2).padStart(5, "0")}`;
}

/** Sizes the backing store to the device pixel ratio and returns the CSS-pixel
 *  box. 1px beat lines blur into 2px mush on HiDPI without this. */
function prepareCanvas(
  canvas: HTMLCanvasElement,
  cssHeight: number,
): { ctx: CanvasRenderingContext2D; width: number; height: number } | null {
  const ctx = canvas.getContext("2d");
  if (!ctx) return null;
  const dpr = window.devicePixelRatio || 1;
  const width = canvas.clientWidth;
  const targetW = Math.max(1, Math.round(width * dpr));
  const targetH = Math.max(1, Math.round(cssHeight * dpr));
  if (canvas.width !== targetW || canvas.height !== targetH) {
    canvas.width = targetW;
    canvas.height = targetH;
  }
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, width, cssHeight);
  return { ctx, width, height: cssHeight };
}

function drawPeaks(
  ctx: CanvasRenderingContext2D,
  peaks: Peaks,
  width: number,
  height: number,
  color: string,
) {
  const mid = height / 2;
  const scale = height / 2;
  ctx.fillStyle = color;
  const n = Math.min(peaks.min.length, width);
  for (let x = 0; x < n; x++) {
    const top = mid - peaks.max[x] * scale;
    const bottom = mid - peaks.min[x] * scale;
    ctx.fillRect(x, top, 1, Math.max(1, bottom - top));
  }
}

function drawGrid(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  bpm: number,
  offset: number,
  fromTime: number,
  toTime: number,
) {
  if (bpm <= 0) return;
  const interval = 60 / bpm;
  const span = toTime - fromTime;
  if (span <= 0) return;
  const pxPerBeat = (interval / span) * width;
  if (pxPerBeat < MIN_GRID_SPACING_PX) return;

  ctx.strokeStyle = GRID_COLOR;
  ctx.lineWidth = 1;
  ctx.beginPath();
  const first = Math.max(0, Math.ceil((fromTime - offset) / interval));
  for (let n = first; ; n++) {
    const t = offset + n * interval;
    if (t > toTime) break;
    // +0.5 puts the stroke on a pixel center so a 1px line stays 1px.
    const x = Math.round(((t - fromTime) / span) * width) + 0.5;
    ctx.moveTo(x, 0);
    ctx.lineTo(x, height);
  }
  ctx.stroke();
}

export default function WaveformView({
  samples,
  sampleRate,
  startSeconds,
  bpm,
  offset,
}: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const overviewRef = useRef<HTMLCanvasElement>(null);
  const detailRef = useRef<HTMLCanvasElement>(null);
  const frameRef = useRef<number | null>(null);

  const [width, setWidth] = useState(0);
  const [playhead, setPlayhead] = useState(0);
  const [playing, setPlaying] = useState(false);
  // Lazy initializer rather than a restore-on-mount effect: this component only
  // ever mounts after an analysis completes, well past hydration, so reading
  // localStorage during its first render can't cause a server/client mismatch.
  const [volume, setVolume] = useState(() => savedOr("volume", 1));
  const { resolvedTheme } = useTheme();

  const duration = samples.length / sampleRate;

  // One player per mounted view. The caller remounts this component (via
  // `key`) for each new analysis, so there is no in-place reload to handle —
  // which keeps transport state out of effect-driven resets.
  const [player] = useState(() => new BeatPlayer());

  useEffect(() => {
    player.load(samples, sampleRate);
    return () => player.dispose();
  }, [player, samples, sampleRate]);

  useEffect(() => {
    player.setGrid(bpm, offset);
  }, [player, bpm, offset]);

  // The click is the whole point of playback here, so it's always on; use the
  // volume slider to pull the pair down.
  useEffect(() => {
    player.setClickEnabled(true);
  }, [player]);

  useEffect(() => {
    player.setVolume(volume);
  }, [player, volume]);

  // Track the canvas width so peaks are recomputed on resize, not stretched.
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    // ResizeObserver fires once on observe(), which covers the initial width —
    // no synchronous setState needed here.
    const observer = new ResizeObserver(() => setWidth(el.clientWidth));
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  // The expensive reduction: whole-track min/max, once per (file, width).
  const overviewPeaks = useMemo(
    () => (width > 0 ? computePeaks(samples, width) : null),
    [samples, width],
  );

  const detailFrom = Math.max(0, Math.min(playhead - DETAIL_SPAN_S / 2, duration - DETAIL_SPAN_S));

  const draw = useCallback(() => {
    const container = containerRef.current;
    const overview = overviewRef.current;
    const detail = detailRef.current;
    if (!container || !overview || !detail || !overviewPeaks) return;
    const palette = readPalette(container);
    const head = player.currentTime;
    const from = Math.max(0, Math.min(head - DETAIL_SPAN_S / 2, duration - DETAIL_SPAN_S));
    const to = from + DETAIL_SPAN_S;

    const ov = prepareCanvas(overview, OVERVIEW_HEIGHT);
    if (ov) {
      // Detail window indicator, drawn under the waveform.
      if (duration > 0) {
        ov.ctx.fillStyle = palette.window;
        const wx = (from / duration) * ov.width;
        const ww = Math.max(2, (DETAIL_SPAN_S / duration) * ov.width);
        ov.ctx.fillRect(wx, 0, ww, ov.height);
      }
      drawPeaks(ov.ctx, overviewPeaks, ov.width, ov.height, palette.wave);
      drawGrid(ov.ctx, ov.width, ov.height, bpm, offset, 0, duration);
      if (duration > 0) {
        ov.ctx.strokeStyle = PLAYHEAD_COLOR;
        ov.ctx.lineWidth = 1;
        ov.ctx.beginPath();
        const hx = Math.round((head / duration) * ov.width) + 0.5;
        ov.ctx.moveTo(hx, 0);
        ov.ctx.lineTo(hx, ov.height);
        ov.ctx.stroke();
      }
    }

    const dt = prepareCanvas(detail, DETAIL_HEIGHT);
    if (dt) {
      const peaks = computePeaks(
        samples,
        dt.width,
        Math.floor(from * sampleRate),
        Math.ceil(to * sampleRate),
      );
      drawPeaks(dt.ctx, peaks, dt.width, dt.height, palette.wave);
      drawGrid(dt.ctx, dt.width, dt.height, bpm, offset, from, to);
      dt.ctx.strokeStyle = PLAYHEAD_COLOR;
      dt.ctx.lineWidth = 1;
      dt.ctx.beginPath();
      const hx = Math.round(((head - from) / DETAIL_SPAN_S) * dt.width) + 0.5;
      dt.ctx.moveTo(hx, 0);
      dt.ctx.lineTo(hx, dt.height);
      dt.ctx.stroke();
    }
  }, [overviewPeaks, samples, sampleRate, duration, bpm, offset, player]);

  // Redraw on any static change (grid, size, theme, seek).
  useEffect(() => {
    draw();
  }, [draw, playhead, resolvedTheme]);

  // While playing, drive both the redraw and the playhead readout off rAF.
  useEffect(() => {
    if (!playing) return;
    const tick = () => {
      if (!player.isPlaying) {
        setPlaying(false);
        setPlayhead(player.currentTime);
        return;
      }
      setPlayhead(player.currentTime);
      draw();
      frameRef.current = requestAnimationFrame(tick);
    };
    frameRef.current = requestAnimationFrame(tick);
    return () => {
      if (frameRef.current !== null) cancelAnimationFrame(frameRef.current);
      frameRef.current = null;
    };
  }, [playing, player, draw]);

  const togglePlay = useCallback(() => {
    if (player.isPlaying) {
      player.pause();
      setPlaying(false);
      setPlayhead(player.currentTime);
    } else {
      void player.play().then(() => setPlaying(player.isPlaying));
    }
  }, [player]);

  const seekFromEvent = useCallback(
    (clientX: number, el: HTMLElement, fromTime: number, span: number) => {
      const rect = el.getBoundingClientRect();
      const frac = Math.max(0, Math.min(1, (clientX - rect.left) / rect.width));
      const t = fromTime + frac * span;
      player.seek(t);
      setPlayhead(player.currentTime);
    },
    [player],
  );

  return (
    <div ref={containerRef} className="flex flex-col gap-2">
      <canvas
        ref={overviewRef}
        style={{ height: OVERVIEW_HEIGHT }}
        className="w-full cursor-pointer rounded-lg border border-border"
        onPointerDown={(e) => {
          e.currentTarget.setPointerCapture(e.pointerId);
          seekFromEvent(e.clientX, e.currentTarget, 0, duration);
        }}
        onPointerMove={(e) => {
          if (e.buttons === 1) seekFromEvent(e.clientX, e.currentTarget, 0, duration);
        }}
      />
      <canvas
        ref={detailRef}
        style={{ height: DETAIL_HEIGHT }}
        className="w-full cursor-pointer rounded-lg border border-border"
        onPointerDown={(e) =>
          seekFromEvent(e.clientX, e.currentTarget, detailFrom, DETAIL_SPAN_S)
        }
      />

      <div className="flex flex-wrap items-center gap-3 text-xs text-muted-foreground">
        <Button type="button" variant="outline" size="xs" onClick={togglePlay}>
          {playing ? "Pause" : "Play"}
        </Button>
        <span className="font-mono tabular-nums">
          {formatTime(startSeconds + playhead)} / {formatTime(startSeconds + duration)}
        </span>
        <div className="flex items-center gap-1.5">
          <Label htmlFor="waveform-volume" className="text-xs font-normal">
            Volume
          </Label>
          <input
            id="waveform-volume"
            type="range"
            min={0}
            max={1}
            step={0.01}
            value={volume}
            onChange={(e) => {
              const v = Number(e.target.value);
              setVolume(v);
              persistOption("volume", v);
            }}
            className="h-1 w-24 cursor-pointer appearance-none rounded-full bg-border accent-foreground"
          />
        </div>
      </div>
    </div>
  );
}
