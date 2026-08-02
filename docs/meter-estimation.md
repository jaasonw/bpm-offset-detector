# Meter (time signature) estimation

Status: **experimental**. The meter stage estimates the beats-per-bar
grouping of a song after BPM and offset detection. It is a heuristic
hint, not ground truth — meter estimation from audio is an open research
problem, and even good systems make mistakes on syncopated or
weakly-accented music.

Only the beats-per-bar *grouping* is estimated (2, 3, 4, 6, or 12). The
denominator is notational, not acoustic: 4/4 vs 2/2 and 6/8 vs 3/4
cannot be distinguished from audio alone.

## Origin

This feature does **not** come from the reference paper. The paper
(*Non-causal Beat Tracking for Rhythm Games*, Bram van de Wetering,
2016) covers only onset extraction, BPM estimation, and offset
estimation — a full-text search finds zero mentions of "meter" or "time
signature", and its output is strictly a BPM/offset pair. The reference
C++ has no meter code. The meter stage was added later as an experiment
(Task 14 in the implementation plan), and the autocorrelation scoring
(Task 15) is a custom design.

For orientation, the component origins across the whole pipeline:

| Component | Origin |
|---|---|
| Onset detection (complex-domain ODF) | Paper §3.2 (reimplemented — the C++ delegated to aubio) |
| BPM scan, gap-confidence, constant-weight voting | Paper §3.3 (faithful port) |
| Offset "off-beat reduction" (`adjust_for_offbeats`) | Paper §3.4.2 (kept) |
| Offset *initial* phase estimate | Paper §3.4.1 — **replaced** by the slopes scan |
| Wider BPM range (40–260), gapless MP3 decoding | Custom |
| /3 subharmonic preference, 4:3 demotion, 3:2 promotion | Custom (driven by real-song regression tests) |
| **Meter estimation, including the autocorrelation scoring** | **Custom, entirely** |

## How the heuristic works

The meter stage runs after BPM/offset detection and receives three
inputs: the **beat grid** (beat positions implied by the winning BPM +
offset), the **onsets** (positions + strengths), and a **triplet_feel
flag** (whether the /3 subharmonic flip fired during BPM detection).

### Step 1 — Turn each beat into an "accent score"

For every beat position on the grid, sum the strengths of all onsets
within ±8% of the beat interval around it (`beat_accents`). In most
music the first beat of a bar is played harder, so the accent sequence
repeats with period = the meter.

Fallback: if all onset strengths are identical (hand-built data), use
the waveform's leading-edge energy at each beat instead.

Example for a 4/4 rock beat (kick hit harder on beat 1):

```
beat:     1    2    3    4    1    2    3    4   ...
accents: [3.0, 2.0, 2.5, 2.0, 3.0, 2.0, 2.5, 2.0, ...]
```

### Step 2 — Mean-center and measure total energy

Subtract the mean from the sequence, then compute the sum of squares
(the zero-lag energy). Pure noise scores ~0 at every lag in the next
step, so no grouping can win by luck. If the energy is ~0 (completely
uniform accents), there is no structure to find and the stage returns
"no estimate".

### Step 3 — Autocorrelation at five candidate lags

For each candidate grouping g ∈ {2, 3, 4, 6, 12}, compute the
mean-centered autocorrelation at lag g, normalized by the zero-lag
energy (score in [−1, 1]).

A period-g pattern correlates strongly at lag g — but also at multiples
of g. This "harmonic leakage" matters in step 4. Noise scores ~0
everywhere. This replaced the original best-phase-contrast scorer, which
took the max over g phase means — a multiple-comparison contest the
largest grouping (12) almost always won on real (noisy) accent data,
reporting 12/8 for every song regardless of true meter.

### Step 4 — Pick the winner: smallest lag near the max

The winner is the **smallest lag whose score is within 0.05 of the best
score** (`TIE_MARGIN`). Because of harmonic leakage, lag-12 often *ties*
lag-4 on a true 4/4 song; the fundamental (4) is musically meaningful,
the multiple (12) is not, so ties break small.

**This is where the main remaining bug lives.** If a song has
phrase-level structure (every 3rd bar accented harder), lag-12 doesn't
just tie lag-4 — it *beats* it by more than 0.05, and the tie-break
can't save the 4/4 reading.

### Step 5 — Abstain if the winner is weak

If the winning score is below **0.3** (`CONFIDENCE_FLOOR`), report
`"unknown"` (`beats_per_bar: 0`) instead of guessing. In much of pop,
drums hit every beat with near-equal strength and bar accents live in
the bass/harmony/vocals, which this feature cannot hear. Abstention is
honest behavior, not a bug.

### Step 6 — Ambiguity flag

If a *different* grouping family (not a multiple of the winner) scored
≥ 80% as well (`AMBIGUITY_RATIO`) — the classic case is 6/8 vs 3/4 —
the estimate is marked `ambiguous`.

### Step 7 — Map to notation

- g=2 → "2/4", g=3 → "3/4", g=4 → "4/4", g=6 → "6/8", g=12 → "12/8"
- **Triplet refinement**: when the BPM stage's /3 flip fired
  (triplet_feel), the beat unit is compound, so g=2 prints as "6/8" and
  g=4 as "12/8".

## Failure modes (observed on real maps)

| Symptom | Cause |
|---|---|
| 4/4 reported as 12/8 | Step 4: lag-12 beat lag-4 by >0.05 via phrase-structure bonus on top of harmonic leakage |
| 4/4 reported as 6/8 | Same, at lag 6 — a 6-beat phrase cycle outscored the 4-beat bar |
| 4/4 reported as 2/4 | No strong 4-beat cycle in the accents; strong-weak-strong-weak reads as 2. Musically defensible; differs from mapper convention |
| "unknown" on most pop | Step 5: accents too flat — honest abstention |

## Proposed improvement (pending)

**Incremental scoring.** Autocorrelation has harmonic accumulation: a
4/4 accent pattern scores at lag-4 *and* at lag-12. The fix is to make
each composite lag earn its size over its own divisors:

```
inc(4)  = score(4) − score(2)
inc(6)  = score(6) − max(score(2), score(3))
inc(12) = score(12) − max(score(2), score(3), score(4), score(6))
```

A larger grouping only wins if its increment over its divisors clears a
margin — it must add *real* structure, not just inherited harmonic
leakage. Confidence becomes the increment (better calibrated than the
raw score: barely-over-floor commits become abstentions). The margin
would be tuned on per-lag score diagnostics across the real-map
regression set.

## Ground-truth caveat (osu! maps)

When evaluating against osu! maps, note that the mapper's meter is the
weakest of the three ground truths: the osu! editor defaults meter to 4
and some mappers never change it, and swung songs are often notated as
plain 4/4 with the beat snap divisor handling the swing. BPM and offset
are what mappers had to get right for the map to be playable; meter is
frequently a default.
