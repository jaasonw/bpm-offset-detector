//! `tempo-cli`: native command-line frontend for `tempo-core`. Decodes
//! audio files with `symphonia` (pure Rust, no system libraries) and runs
//! the tempo/offset detection pipeline.

mod decode;

use std::fs;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde::Serialize;
use tempo_core::{detect, DetectOptions, TempoResult};

use decode::decode_audio_file;

/// File extensions `symphonia` (as configured in this crate's Cargo.toml)
/// can decode; used to filter directory entries in `batch` mode.
const SUPPORTED_EXTENSIONS: &[&str] = &["mp3", "wav", "flac", "ogg", "aac", "m4a"];

#[derive(Parser)]
#[command(name = "tempo", about = "Detects BPM and beat offset of audio files")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Path to an audio file to analyze (ignored if a subcommand is given).
    file: Option<PathBuf>,

    #[command(flatten)]
    options: SharedOptions,
}

#[derive(Parser, Clone)]
struct SharedOptions {
    /// Slowest BPM to consider.
    #[arg(long, default_value_t = 89.0)]
    min_bpm: f64,
    /// Fastest BPM to consider.
    #[arg(long, default_value_t = 205.0)]
    max_bpm: f64,
    /// Start offset into the audio, in seconds.
    #[arg(long, default_value_t = 0.0)]
    start: f64,
    /// Duration of audio to analyze, in seconds.
    #[arg(long, default_value_t = 60.0)]
    duration: f64,
    /// Print results as JSON instead of human-readable text.
    #[arg(long)]
    json: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Analyze every supported audio file in a folder and write a
    /// CSV (or, with --json, a JSON array) of results.
    Batch {
        folder: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[command(flatten)]
        options: SharedOptions,
    },
}

#[derive(Serialize)]
struct ResultOut {
    bpm: f64,
    offset: f64,
    fitness: f64,
}

impl From<TempoResult> for ResultOut {
    fn from(r: TempoResult) -> Self {
        ResultOut {
            bpm: r.bpm,
            offset: r.offset,
            fitness: r.fitness,
        }
    }
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Some(Command::Batch {
            folder,
            out,
            options,
        }) => run_batch(&folder, &out, &options),
        None => match cli.file {
            Some(file) => run_single(&file, &cli.options),
            None => {
                eprintln!("error: no input file given");
                eprintln!("usage: tempo <file> [options]");
                eprintln!("       tempo batch <folder> --out <results.csv> [options]");
                std::process::exit(2);
            }
        },
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn detect_options_from(shared: &SharedOptions) -> DetectOptions {
    DetectOptions {
        min_bpm: shared.min_bpm,
        max_bpm: shared.max_bpm,
    }
}

/// Slices `samples` to the `[start, start + duration)` second range implied
/// by `shared`, clamped to the available audio.
fn slice_to_range<'a>(samples: &'a [f32], sample_rate: u32, shared: &SharedOptions) -> &'a [f32] {
    let start_frame = ((shared.start.max(0.0)) * sample_rate as f64) as usize;
    let start_frame = start_frame.min(samples.len());
    let len_frames = (shared.duration.max(0.0) * sample_rate as f64) as usize;
    let end_frame = (start_frame + len_frames).min(samples.len());
    &samples[start_frame..end_frame]
}

fn run_single(
    file: &std::path::Path,
    options: &SharedOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let (samples, sample_rate) = decode_audio_file(file)?;
    let slice = slice_to_range(&samples, sample_rate, options);
    let results = detect(slice, sample_rate, &detect_options_from(options));

    if options.json {
        let out: Vec<ResultOut> = results.into_iter().map(ResultOut::from).collect();
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        for r in &results {
            println!(
                "[RESULT] {:.3} BPM, offset @ {:.3} sec, fitness {:.3}",
                r.bpm, r.offset, r.fitness
            );
        }
    }

    Ok(())
}

fn run_batch(
    folder: &std::path::Path,
    out: &std::path::Path,
    options: &SharedOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    #[derive(Serialize)]
    struct BatchRow {
        file: String,
        rank: usize,
        bpm: f64,
        offset: f64,
        fitness: f64,
    }

    let mut rows = Vec::new();

    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        let path = entry.path();
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if !SUPPORTED_EXTENSIONS.contains(&ext.to_lowercase().as_str()) {
            continue;
        }

        let file_name = path.display().to_string();
        eprintln!("analyzing {file_name}...");

        let (samples, sample_rate) = match decode_audio_file(&path) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("  skipping {file_name}: {e}");
                continue;
            }
        };
        let slice = slice_to_range(&samples, sample_rate, options);
        let results = detect(slice, sample_rate, &detect_options_from(options));

        for (i, r) in results.into_iter().enumerate() {
            rows.push(BatchRow {
                file: file_name.clone(),
                rank: i + 1,
                bpm: r.bpm,
                offset: r.offset,
                fitness: r.fitness,
            });
        }
    }

    if options.json {
        let json = serde_json::to_string_pretty(&rows)?;
        fs::write(out, json)?;
    } else {
        let mut writer = csv::Writer::from_path(out)?;
        writer.write_record(["file", "rank", "bpm", "offset", "fitness"])?;
        for row in &rows {
            writer.write_record([
                row.file.clone(),
                row.rank.to_string(),
                row.bpm.to_string(),
                row.offset.to_string(),
                row.fitness.to_string(),
            ])?;
        }
        writer.flush()?;
    }

    Ok(())
}
