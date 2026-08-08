//! `osu-eval`: compares tempo-cli's BPM/offset/meter analysis against
//! human mapper ground truth extracted from osu! beatmapsets (`.osz`).
//!
//! Local development tool, not part of CI: point it at a folder of `.osz`
//! files (gitignored — they bundle copyrighted audio, so they can never
//! be committed). Emits one CSV row per analyzed map plus console summary
//! statistics; the offset-error distribution across many maps is how we
//! calibrate the detector's systematic offset bias with real confidence.
//!
//! Usage: `osu-eval <folder> [--out report.csv] [--min-bpm 40]
//! [--max-bpm 260] [--start 0] [--duration 60]`

use std::path::{Path, PathBuf};

use clap::Parser;
use tempo_cli::decode::decode_audio_file;
use tempo_cli::osu::{extract_audio, read_osz_timings, Skip};
use tempo_core::{detect_with_onsets, estimate_meter, DetectOptions};

#[derive(Parser)]
#[command(
    name = "osu-eval",
    about = "Compare detection against osu! mapper ground truth over a folder of .osz files"
)]
struct Cli {
    /// Folder containing .osz files.
    folder: PathBuf,

    /// Write the per-map CSV report here instead of stdout.
    #[arg(long)]
    out: Option<PathBuf>,

    /// Print a compact, human-readable table to stdout instead of CSV.
    #[arg(long, conflicts_with = "out")]
    table: bool,

    /// Slowest BPM to consider.
    #[arg(long, default_value_t = 40.0)]
    min_bpm: f64,
    /// Fastest BPM to consider.
    #[arg(long, default_value_t = 260.0)]
    max_bpm: f64,
    /// Start offset into the audio, in seconds.
    #[arg(long, default_value_t = 0.0)]
    start: f64,
    /// Duration of audio to analyze, in seconds.
    #[arg(long, default_value_t = 60.0)]
    duration: f64,
}

/// One row of the CSV report. Numeric fields are empty for skipped maps.
#[derive(serde::Serialize)]
struct Row {
    osz: String,
    audio: String,
    status: String,
    true_bpm: String,
    detected_bpm: String,
    bpm_error: String,
    /// Which rank (1-3) the true BPM appears at, empty when absent.
    true_bpm_rank: String,
    true_offset_ms: String,
    detected_offset_ms: String,
    /// Signed, wrapped into +-half a beat; empty when the BPM is wrong
    /// (an offset error on the wrong grid is meaningless).
    offset_error_ms: String,
    meter_true: String,
    meter_detected: String,
    meter_confidence: String,
}

impl Row {
    fn skipped(osz: &str, reason: &str) -> Self {
        Row {
            osz: osz.to_string(),
            audio: String::new(),
            status: format!("skipped: {reason}"),
            true_bpm: String::new(),
            detected_bpm: String::new(),
            bpm_error: String::new(),
            true_bpm_rank: String::new(),
            true_offset_ms: String::new(),
            detected_offset_ms: String::new(),
            offset_error_ms: String::new(),
            meter_true: String::new(),
            meter_detected: String::new(),
            meter_confidence: String::new(),
        }
    }
}

fn fmt_f(v: f64, digits: usize) -> String {
    format!("{v:.digits$}")
}

/// Max map-name width in table output (longer names are truncated with …).
const TABLE_MAP_WIDTH: usize = 45;

/// Prints rows as a compact, human-readable table: the CSV's wide
/// 13-column schema is narrowed by dropping the audio filename (already
/// implied by the map name) and merging the two meter columns into
/// `true→detected`. Numeric columns are right-aligned.
fn print_table(rows: &[Row]) {
    let headers = [
        "map", "status", "t_bpm", "d_bpm", "err", "r", "t_off", "d_off", "off_err", "meter",
    ];
    let right = [
        false, false, true, true, true, true, true, true, true, false,
    ];
    const COLS: usize = 10;

    let truncate = |s: &str| -> String {
        if s.chars().count() <= TABLE_MAP_WIDTH {
            s.to_string()
        } else {
            format!(
                "{}…",
                s.chars().take(TABLE_MAP_WIDTH - 1).collect::<String>()
            )
        }
    };

    let mut data: Vec<[String; COLS]> = Vec::new();
    for r in rows {
        let meter = if r.meter_true.is_empty() {
            String::new()
        } else {
            format!(
                "{}→{}",
                r.meter_true,
                if r.meter_detected.is_empty() {
                    "-"
                } else {
                    &r.meter_detected
                }
            )
        };
        data.push([
            truncate(&r.osz),
            r.status.clone(),
            r.true_bpm.clone(),
            r.detected_bpm.clone(),
            r.bpm_error.clone(),
            r.true_bpm_rank.clone(),
            r.true_offset_ms.clone(),
            r.detected_offset_ms.clone(),
            r.offset_error_ms.clone(),
            meter,
        ]);
    }

    let mut widths = [0usize; COLS];
    for (i, h) in headers.iter().enumerate() {
        widths[i] = h.len();
    }
    for row in &data {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }

    let render = |cells: &[String; COLS]| -> String {
        let mut line = String::new();
        for (i, cell) in cells.iter().enumerate() {
            if right[i] {
                line.push_str(&format!("{:>width$}  ", cell, width = widths[i]));
            } else {
                line.push_str(&format!("{:<width$}  ", cell, width = widths[i]));
            }
        }
        line.trim_end().to_string()
    };

    let header_owned: [String; COLS] = headers.map(String::from);
    let header_line = render(&header_owned);
    println!("{header_line}");
    println!("{}", "-".repeat(header_line.chars().count()));
    for row in &data {
        println!("{}", render(row));
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    let opts = DetectOptions {
        min_bpm: cli.min_bpm,
        max_bpm: cli.max_bpm,
        ..Default::default()
    };

    let mut osz_files: Vec<PathBuf> = std::fs::read_dir(&cli.folder)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("osz"))
        })
        .collect();
    osz_files.sort();

    if osz_files.is_empty() {
        eprintln!("no .osz files found in {}", cli.folder.display());
        std::process::exit(1);
    }

    let writer: Box<dyn std::io::Write> = match &cli.out {
        Some(path) => Box::new(std::fs::File::create(path)?),
        None => Box::new(std::io::stdout()),
    };
    let mut csv = csv::Writer::from_writer(writer);
    let mut rows: Vec<Row> = Vec::new();

    let temp_dir = std::env::temp_dir().join(format!("osu-eval-{}", std::process::id()));
    std::fs::create_dir_all(&temp_dir)?;

    let mut analyzed = 0usize;
    let mut bpm_correct = 0usize;
    let mut offset_errors_ms: Vec<f64> = Vec::new();
    let mut skip_reasons: Vec<(String, String)> = Vec::new();
    // Audio-content hashes already analyzed, to dedupe sets downloaded
    // twice (e.g. video and no-video variants of the same beatmapset).
    let mut seen_audio: std::collections::HashMap<u64, String> = std::collections::HashMap::new();

    for osz in &osz_files {
        let osz_name = osz
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();

        let timings = match read_osz_timings(osz) {
            Ok(t) => t,
            Err(Skip::Io(msg)) => {
                rows.push(Row::skipped(&osz_name, &format!("io: {msg}")));
                skip_reasons.push((osz_name, format!("io: {msg}")));
                continue;
            }
            Err(reason) => {
                rows.push(Row::skipped(&osz_name, &reason.to_string()));
                skip_reasons.push((osz_name, reason.to_string()));
                continue;
            }
        };

        for timing in &timings {
            let audio_path = match extract_audio(osz, &timing.audio_filename, &temp_dir) {
                Ok(p) => p,
                Err(e) => {
                    let msg = format!("audio extraction failed: {e}");
                    rows.push(Row::skipped(&osz_name, &msg));
                    skip_reasons.push((osz_name.clone(), msg));
                    continue;
                }
            };
            let hash = hash_file(&audio_path);
            if let Some(first) = seen_audio.get(&hash) {
                let msg = format!("duplicate audio of {first}");
                rows.push(Row::skipped(&osz_name, &msg));
                skip_reasons.push((osz_name.clone(), msg));
                continue;
            }
            seen_audio.insert(hash, osz_name.clone());

            let result = analyze(&audio_path, timing, &opts, &cli, &osz_name);
            match result {
                Ok((row, offset_err, bpm_ok)) => {
                    if bpm_ok {
                        bpm_correct += 1;
                    }
                    if let Some(err) = offset_err {
                        offset_errors_ms.push(err);
                    }
                    rows.push(row);
                }
                Err(msg) => {
                    rows.push(Row::skipped(&osz_name, &msg));
                    skip_reasons.push((osz_name.clone(), msg));
                    continue;
                }
            }
            analyzed += 1;
        }
    }

    std::fs::remove_dir_all(&temp_dir).ok();

    if cli.table {
        print_table(&rows);
    } else {
        for row in &rows {
            csv.serialize(row)?;
        }
        csv.flush()?;
    }

    // Summary statistics on stderr so stdout stays pure CSV.
    eprintln!();
    eprintln!(
        "== summary: {} analyzed, {} skipped, {} total .osz ==",
        analyzed,
        skip_reasons.len(),
        osz_files.len()
    );
    for (name, reason) in &skip_reasons {
        eprintln!("  skipped {name}: {reason}");
    }
    if analyzed > 0 {
        eprintln!(
            "bpm: {}/{} within 0.5 BPM of ground truth",
            bpm_correct, analyzed
        );
    }
    if !offset_errors_ms.is_empty() {
        let mut sorted = offset_errors_ms.clone();
        sorted.sort_by(f64::total_cmp);
        let mean = sorted.iter().sum::<f64>() / sorted.len() as f64;
        let median = sorted[sorted.len() / 2];
        let variance = sorted.iter().map(|e| (e - mean).powi(2)).sum::<f64>() / sorted.len() as f64;
        let within_10 = sorted.iter().filter(|e| e.abs() <= 10.0).count();
        let within_40 = sorted.iter().filter(|e| e.abs() <= 40.0).count();
        eprintln!(
            "offset error (ms, signed, BPM-correct maps only): \
             mean {mean:+.1}, median {:+.1}, stddev {:.1}, min {:+.1}, max {:+.1}",
            median,
            variance.sqrt(),
            sorted.first().unwrap(),
            sorted.last().unwrap()
        );
        eprintln!(
            "offset accuracy: {}/{} within +-10ms, {}/{} within +-40ms",
            within_10,
            sorted.len(),
            within_40,
            sorted.len()
        );
    }

    Ok(())
}

/// Hashes a file's contents for duplicate-audio detection (SipHash via
/// DefaultHasher — non-cryptographic, collision risk is negligible at
/// this scale).
fn hash_file(path: &Path) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if let Ok(bytes) = std::fs::read(path) {
        hasher.write(&bytes);
    }
    hasher.finish()
}

/// Analyzes one map's audio against its timing ground truth. Returns the
/// CSV row, the signed offset error in ms (only when the BPM matched), and
/// whether the BPM matched.
fn analyze(
    audio_path: &Path,
    timing: &tempo_cli::osu::OsuTiming,
    opts: &DetectOptions,
    cli: &Cli,
    osz_name: &str,
) -> Result<(Row, Option<f64>, bool), String> {
    let (samples, sample_rate) =
        decode_audio_file(audio_path).map_err(|e| format!("decode failed: {e}"))?;

    // Slice to [start, start + duration) like the main CLI.
    let start_frame = ((cli.start.max(0.0)) * sample_rate as f64) as usize;
    let start_frame = start_frame.min(samples.len());
    let len_frames = ((cli.duration.max(0.0)) * sample_rate as f64) as usize;
    let end_frame = (start_frame + len_frames).min(samples.len());
    let slice = &samples[start_frame..end_frame];

    let (onsets, results, context) = detect_with_onsets(slice, sample_rate, opts);
    let Some(top) = results.first() else {
        return Err("detector returned no candidates".to_string());
    };
    let meter = estimate_meter(&onsets, slice, sample_rate, top.bpm, top.offset, &context);

    let bpm_err = top.bpm - timing.bpm;
    let bpm_ok = bpm_err.abs() < 0.5;
    let true_bpm_rank = results
        .iter()
        .position(|r| (r.bpm - timing.bpm).abs() < 0.5)
        .map(|i| (i + 1).to_string())
        .unwrap_or_default();

    // Signed offset error, wrapped into +-half a beat so a detector phase
    // near the interval boundary doesn't read as a full-beat error.
    let offset_err_ms = if bpm_ok {
        let interval_s = 60.0 / timing.bpm;
        let true_mod_s = (timing.offset_ms / 1000.0).rem_euclid(interval_s);
        let mut err = top.offset - true_mod_s;
        err = (err + interval_s / 2.0).rem_euclid(interval_s) - interval_s / 2.0;
        Some(err * 1000.0)
    } else {
        None
    };

    let row = Row {
        osz: osz_name.to_string(),
        audio: timing.audio_filename.clone(),
        status: "ok".to_string(),
        true_bpm: fmt_f(timing.bpm, 3),
        detected_bpm: fmt_f(top.bpm, 3),
        bpm_error: fmt_f(bpm_err, 3),
        true_bpm_rank,
        true_offset_ms: fmt_f(timing.offset_ms, 1),
        detected_offset_ms: fmt_f(top.offset * 1000.0, 1),
        offset_error_ms: offset_err_ms.map(|e| fmt_f(e, 1)).unwrap_or_default(),
        meter_true: timing.meter.to_string(),
        meter_detected: meter
            .as_ref()
            .map(|m| m.notation.clone())
            .unwrap_or_default(),
        meter_confidence: meter.map(|m| fmt_f(m.confidence, 2)).unwrap_or_default(),
    };

    Ok((row, offset_err_ms, bpm_ok))
}
