// Plays back the exact mono slice that was analyzed, plus an optional
// metronome click on the candidate's beat grid.
//
// Playback and clicks are scheduled against one AudioContext clock, so they
// cannot drift relative to each other — which is the whole point: judging an
// offset by ear only works if the click is sample-accurate against the audio.
// Times here are all relative to the start of the analyzed slice, matching the
// `offset` values tempo-core returns.

const LOOKAHEAD_MS = 25;
const SCHEDULE_AHEAD_S = 0.15;
const CLICK_FREQ_HZ = 1000;
const CLICK_DURATION_S = 0.03;

export class BeatPlayer {
  private ctx: AudioContext | null = null;
  private buffer: AudioBuffer | null = null;
  private source: AudioBufferSourceNode | null = null;
  private timer: ReturnType<typeof setInterval> | null = null;

  private pendingSamples: { samples: Float32Array; sampleRate: number } | null = null;

  /** Media time (seconds into the slice) the current source was started at. */
  private startedAtMediaTime = 0;
  /** ctx.currentTime when the current source was started. */
  private startedAtContextTime = 0;
  private playing = false;
  private pausedAt = 0;

  private bpm = 0;
  private offset = 0;
  private clickEnabled = false;
  /** Index of the next beat not yet scheduled. */
  private nextBeat = 0;

  /** Buffers PCM; the AudioContext itself is created on the first play() so it
   *  is constructed inside a user gesture and not left suspended. */
  load(samples: Float32Array, sampleRate: number) {
    this.stop();
    this.buffer = null;
    this.pendingSamples = { samples, sampleRate };
    this.pausedAt = 0;
    if (this.ctx) this.materialize();
  }

  private materialize() {
    const ctx = this.ctx;
    const pending = this.pendingSamples;
    if (!ctx || !pending || this.buffer) return;
    const buf = ctx.createBuffer(1, Math.max(1, pending.samples.length), pending.sampleRate);
    // `set` rather than `copyToChannel`: the latter's lib.dom type demands a
    // Float32Array backed specifically by ArrayBuffer, which our decoded PCM
    // isn't statically known to be.
    buf.getChannelData(0).set(pending.samples);
    this.buffer = buf;
  }

  get duration(): number {
    if (this.buffer) return this.buffer.duration;
    if (this.pendingSamples) {
      return this.pendingSamples.samples.length / this.pendingSamples.sampleRate;
    }
    return 0;
  }

  get isPlaying(): boolean {
    return this.playing;
  }

  /** Current position in seconds from the start of the analyzed slice. */
  get currentTime(): number {
    if (!this.playing || !this.ctx) return this.pausedAt;
    const t = this.startedAtMediaTime + (this.ctx.currentTime - this.startedAtContextTime);
    return Math.min(t, this.duration);
  }

  setGrid(bpm: number, offset: number) {
    this.bpm = bpm;
    this.offset = offset;
    this.resyncClicks();
  }

  setClickEnabled(enabled: boolean) {
    this.clickEnabled = enabled;
    this.resyncClicks();
  }

  /** Drops already-scheduled clicks and re-aims the scheduler at the playhead.
   *  Without this, a grid change mid-playback would keep clicking the old grid
   *  for up to SCHEDULE_AHEAD_S. */
  private resyncClicks() {
    if (!this.playing) return;
    const at = this.currentTime;
    this.stopSource({ keepPlayingState: true });
    this.startSource(at);
  }

  async play(fromSeconds?: number) {
    if (!this.ctx) {
      const Ctor =
        window.AudioContext ||
        (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
      this.ctx = new Ctor();
    }
    if (this.ctx.state === "suspended") await this.ctx.resume();
    this.materialize();
    if (!this.buffer) return;

    const from = fromSeconds ?? this.pausedAt;
    this.startSource(from >= this.duration ? 0 : from);
    this.playing = true;
  }

  pause() {
    if (!this.playing) return;
    const at = this.currentTime;
    this.stopSource();
    this.pausedAt = at;
    this.playing = false;
  }

  seek(seconds: number) {
    const clamped = Math.max(0, Math.min(seconds, this.duration));
    if (this.playing) {
      this.stopSource({ keepPlayingState: true });
      this.pausedAt = clamped;
      this.startSource(clamped);
    } else {
      this.pausedAt = clamped;
    }
  }

  stop() {
    this.stopSource();
    this.playing = false;
    this.pausedAt = 0;
  }

  dispose() {
    this.stopSource();
    this.playing = false;
    this.buffer = null;
    this.pendingSamples = null;
    const ctx = this.ctx;
    this.ctx = null;
    void ctx?.close().catch(() => {
      // Already closed, or closing during teardown — nothing to recover.
    });
  }

  private startSource(from: number) {
    const ctx = this.ctx;
    if (!ctx || !this.buffer) return;
    const src = ctx.createBufferSource();
    src.buffer = this.buffer;
    src.connect(ctx.destination);
    src.onended = () => {
      // Only a natural end should reset transport state; stopSource() detaches
      // this handler first so seeks and pauses don't trip it.
      if (this.source === src) {
        this.source = null;
        this.playing = false;
        this.pausedAt = this.duration;
        this.stopScheduler();
      }
    };
    this.startedAtMediaTime = from;
    this.startedAtContextTime = ctx.currentTime;
    this.pausedAt = from;
    src.start(0, from);
    this.source = src;

    const interval = 60 / (this.bpm > 0 ? this.bpm : 120);
    this.nextBeat = Math.max(0, Math.ceil((from - this.offset) / interval));
    this.startScheduler();
  }

  private stopSource(opts: { keepPlayingState?: boolean } = {}) {
    const src = this.source;
    this.source = null;
    if (src) {
      src.onended = null;
      try {
        src.stop();
      } catch {
        // Never started, or already stopped — nothing to do.
      }
      src.disconnect();
    }
    this.stopScheduler();
    if (!opts.keepPlayingState) this.playing = false;
  }

  private startScheduler() {
    this.stopScheduler();
    if (!this.clickEnabled || this.bpm <= 0) return;
    this.scheduleClicks();
    this.timer = setInterval(() => this.scheduleClicks(), LOOKAHEAD_MS);
  }

  private stopScheduler() {
    if (this.timer !== null) {
      clearInterval(this.timer);
      this.timer = null;
    }
  }

  private scheduleClicks() {
    const ctx = this.ctx;
    if (!ctx || !this.clickEnabled || this.bpm <= 0) return;
    const interval = 60 / this.bpm;
    const horizon = ctx.currentTime + SCHEDULE_AHEAD_S;

    for (;;) {
      const beatMediaTime = this.offset + this.nextBeat * interval;
      if (beatMediaTime > this.duration) return;
      const when =
        this.startedAtContextTime + (beatMediaTime - this.startedAtMediaTime);
      if (when > horizon) return;
      if (when >= ctx.currentTime) this.emitClick(ctx, when);
      this.nextBeat++;
    }
  }

  private emitClick(ctx: AudioContext, when: number) {
    const osc = ctx.createOscillator();
    const gain = ctx.createGain();
    osc.frequency.value = CLICK_FREQ_HZ;
    // Short exponential decay: an unshaped gate on a 1kHz sine clicks at both
    // edges and smears the very transient we're trying to line up against.
    gain.gain.setValueAtTime(0.0001, when);
    gain.gain.exponentialRampToValueAtTime(0.6, when + 0.001);
    gain.gain.exponentialRampToValueAtTime(0.0001, when + CLICK_DURATION_S);
    osc.connect(gain).connect(ctx.destination);
    osc.start(when);
    osc.stop(when + CLICK_DURATION_S + 0.01);
  }
}
