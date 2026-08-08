//! Ground-truth timing extraction from osu! beatmapsets (`.osz`) for the
//! `osu-eval` harness.
//!
//! A `.osz` is a zip archive containing one `.osu` text file per
//! difficulty plus the shared audio file. The mapper's timing is in the
//! `[TimingPoints]` section: lines of
//! `time,beatLength,meter,sampleSet,sampleIndex,volume,uninherited,effects`
//! where an *uninherited* point (`uninherited=1`, positive `beatLength`)
//! defines the tempo: `bpm = 60000 / beatLength`, `offset = time` (ms).
//! The audio file is named by `[General] AudioFilename`.

use std::io::Read;
use std::path::{Path, PathBuf};

/// Timing ground truth extracted from one difficulty's `[TimingPoints]`.
#[derive(Debug, Clone, PartialEq)]
pub struct OsuTiming {
    /// `[General] AudioFilename`: the audio file's path inside the .osz.
    pub audio_filename: String,
    /// `60000 / beatLength` of the first uninherited timing point.
    pub bpm: f64,
    /// `time` of the first uninherited timing point, in milliseconds.
    pub offset_ms: f64,
    /// `meter` of the first uninherited timing point (beats per bar —
    /// osu! has no denominator concept, so this is a weaker ground truth
    /// than BPM/offset, matching the limitation of our own meter feature).
    pub meter: u32,
}

/// Why a beatmap(set) can't serve as ground truth.
#[derive(Debug, Clone, PartialEq)]
pub enum Skip {
    /// `[General]` has no `AudioFilename`.
    NoAudioFilename,
    /// No uninherited timing point exists.
    NoUninheritedTimingPoint,
    /// Uninherited timing points disagree on `beatLength` (tempo changes
    /// mid-song; a single-BPM/offset comparison is meaningless).
    VariableBpm,
    /// The archive contains no `.osu` files at all.
    NoOsuFiles,
    /// Zip or IO failure.
    Io(String),
}

impl std::fmt::Display for Skip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Skip::NoAudioFilename => write!(f, "no AudioFilename"),
            Skip::NoUninheritedTimingPoint => write!(f, "no uninherited timing point"),
            Skip::VariableBpm => write!(f, "variable BPM"),
            Skip::NoOsuFiles => write!(f, "no .osu files in archive"),
            Skip::Io(msg) => write!(f, "io: {msg}"),
        }
    }
}

/// Parses one `.osu` file's content into its timing ground truth.
pub fn parse_osu(content: &str) -> Result<OsuTiming, Skip> {
    let mut section = "";
    let mut audio_filename: Option<String> = None;
    // (time_ms, beat_length, meter) of every uninherited timing point.
    let mut uninherited: Vec<(f64, f64, u32)> = Vec::new();

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = &line[1..line.len() - 1];
            continue;
        }
        match section {
            "General" => {
                if let Some((key, value)) = line.split_once(':') {
                    if key.trim() == "AudioFilename" {
                        audio_filename = Some(value.trim().to_string());
                    }
                }
            }
            "TimingPoints" => {
                let fields: Vec<&str> = line.split(',').collect();
                if fields.len() < 3 {
                    continue;
                }
                let (Ok(time), Ok(beat_length)) = (
                    fields[0].trim().parse::<f64>(),
                    fields[1].trim().parse::<f64>(),
                ) else {
                    continue;
                };
                let meter = fields[2].trim().parse::<u32>().unwrap_or(4);
                // Old-format files may lack the flag entirely; there, any
                // positive beatLength is an uninherited point by definition.
                let is_uninherited = fields.get(6).map_or(beat_length > 0.0, |f| f.trim() == "1");
                if is_uninherited && beat_length > 0.0 {
                    uninherited.push((time, beat_length, meter));
                }
            }
            _ => {}
        }
    }

    let audio_filename = audio_filename.ok_or(Skip::NoAudioFilename)?;
    let Some(&(time, first_beat_length, meter)) = uninherited.first() else {
        return Err(Skip::NoUninheritedTimingPoint);
    };
    // Re-anchored points with the SAME beatLength are fine (the map's
    // tempo is still constant; we use the first point's offset). A real
    // beatLength change means variable tempo.
    if uninherited
        .iter()
        .any(|&(_, bl, _)| (bl - first_beat_length).abs() > 0.01)
    {
        return Err(Skip::VariableBpm);
    }

    Ok(OsuTiming {
        audio_filename,
        bpm: 60000.0 / first_beat_length,
        offset_ms: time,
        meter,
    })
}

/// Reads every `.osu` inside a `.osz` and returns the distinct timing
/// ground truths (difficulties of a set normally share one timing; the
/// dedup keeps each unique audio+timing combo exactly once).
pub fn read_osz_timings(osz_path: &Path) -> Result<Vec<OsuTiming>, Skip> {
    let file = std::fs::File::open(osz_path).map_err(|e| Skip::Io(e.to_string()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| Skip::Io(e.to_string()))?;

    let mut timings: Vec<OsuTiming> = Vec::new();
    let mut first_err: Option<Skip> = None;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| Skip::Io(e.to_string()))?;
        if !entry.name().to_ascii_lowercase().ends_with(".osu") {
            continue;
        }
        let mut content = String::new();
        entry
            .read_to_string(&mut content)
            .map_err(|e| Skip::Io(e.to_string()))?;
        match parse_osu(&content) {
            Ok(timing) => {
                if !timings.contains(&timing) {
                    timings.push(timing);
                }
            }
            Err(e) => {
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }

    if timings.is_empty() {
        return Err(first_err.unwrap_or(Skip::NoOsuFiles));
    }
    Ok(timings)
}

/// Extracts the audio entry referenced by `audio_filename` from the .osz
/// at `osz_path` into `dest_dir`, returning the extracted file's path.
/// Matching tolerates Windows-style `\` separators, subdirectory prefixes
/// inside the archive, and case differences.
pub fn extract_audio(
    osz_path: &Path,
    audio_filename: &str,
    dest_dir: &Path,
) -> Result<PathBuf, Skip> {
    let file = std::fs::File::open(osz_path).map_err(|e| Skip::Io(e.to_string()))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| Skip::Io(e.to_string()))?;
    let wanted = audio_filename.replace('\\', "/");
    let wanted_name = wanted.rsplit('/').next().unwrap_or(&wanted);

    let mut found: Option<String> = None;
    for i in 0..archive.len() {
        let entry = archive.by_index(i).map_err(|e| Skip::Io(e.to_string()))?;
        let name = entry.name().replace('\\', "/");
        let entry_name = name.rsplit('/').next().unwrap_or(&name);
        if name == wanted || entry_name.eq_ignore_ascii_case(wanted_name) {
            found = Some(entry.name().to_string());
            break;
        }
    }
    let entry_name =
        found.ok_or_else(|| Skip::Io(format!("audio '{audio_filename}' not found in archive")))?;

    let mut entry = archive
        .by_name(&entry_name)
        .map_err(|e| Skip::Io(e.to_string()))?;
    let ext = Path::new(&entry_name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin")
        .to_string();
    let dest = dest_dir.join(format!("osu-eval-audio.{ext}"));
    let mut out = std::fs::File::create(&dest).map_err(|e| Skip::Io(e.to_string()))?;
    std::io::copy(&mut entry, &mut out).map_err(|e| Skip::Io(e.to_string()))?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASIC_OSU: &str = "\
osu file format v14

[General]
AudioFilename: song.mp3
Mode: 0

[Metadata]
Title: Test Song

[TimingPoints]
726,333.333,4,2,0,80,1,0
1500,-100,4,2,0,60,0,0

[HitObjects]
";

    #[test]
    fn parses_basic_timing() {
        let timing = parse_osu(BASIC_OSU).unwrap();
        assert_eq!(timing.audio_filename, "song.mp3");
        assert!((timing.bpm - 180.0).abs() < 0.001);
        assert!((timing.offset_ms - 726.0).abs() < 0.001);
        assert_eq!(timing.meter, 4);
    }

    #[test]
    fn ignores_inherited_points_for_tempo() {
        // An inherited (negative beatLength) point before the real one.
        let content = BASIC_OSU.replace(
            "726,333.333,4,2,0,80,1,0",
            "500,-50,4,2,0,80,0,0\n726,333.333,4,2,0,80,1,0",
        );
        let timing = parse_osu(&content).unwrap();
        assert!((timing.offset_ms - 726.0).abs() < 0.001);
    }

    #[test]
    fn old_format_without_uninherited_flag_uses_positive_beat_length() {
        let content = "[General]\nAudioFilename: a.mp3\n\n[TimingPoints]\n1000,500,4,1,0,80\n";
        let timing = parse_osu(content).unwrap();
        assert!((timing.bpm - 120.0).abs() < 0.001);
    }

    #[test]
    fn variable_bpm_is_rejected() {
        let content = BASIC_OSU.replace("1500,-100,4,2,0,60,0,0", "1500,250.0,4,2,0,60,1,0");
        assert_eq!(parse_osu(&content), Err(Skip::VariableBpm));
    }

    #[test]
    fn reanchored_same_bpm_is_accepted() {
        // Two uninherited points with the same beatLength: constant tempo,
        // first point's offset wins.
        let content = BASIC_OSU.replace("1500,-100,4,2,0,60,0,0", "1500,333.333,4,2,0,60,1,0");
        let timing = parse_osu(&content).unwrap();
        assert!((timing.offset_ms - 726.0).abs() < 0.001);
    }

    #[test]
    fn missing_audio_filename_is_rejected() {
        let content = "[TimingPoints]\n726,333.333,4,2,0,80,1,0\n";
        assert_eq!(parse_osu(content), Err(Skip::NoAudioFilename));
    }

    #[test]
    fn missing_uninherited_point_is_rejected() {
        let content = "[General]\nAudioFilename: a.mp3\n\n[TimingPoints]\n726,-100,4,2,0,80,0,0\n";
        assert_eq!(parse_osu(content), Err(Skip::NoUninheritedTimingPoint));
    }

    #[test]
    fn tolerates_crlf_comments_and_blank_lines() {
        let content = BASIC_OSU.replace('\n', "\r\n") + "\r\n// a comment\r\n\r\n";
        let timing = parse_osu(&content).unwrap();
        assert_eq!(timing.audio_filename, "song.mp3");
    }

    /// Writes a minimal .osz (zip with the given entries) to `path`.
    fn write_test_osz(path: &Path, entries: &[(&str, &str)]) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, content) in entries {
            writer.start_file(*name, options).unwrap();
            std::io::Write::write_all(&mut writer, content.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }

    #[test]
    fn read_osz_dedupes_difficulties_sharing_timing() {
        let dir = std::env::temp_dir().join(format!("osu-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let osz = dir.join("set.osz");
        write_test_osz(
            &osz,
            &[
                ("map [hard].osu", BASIC_OSU),
                ("map [insane].osu", BASIC_OSU),
                ("song.mp3", "not real audio"),
            ],
        );

        let timings = read_osz_timings(&osz).unwrap();
        assert_eq!(timings.len(), 1);
        assert!((timings[0].bpm - 180.0).abs() < 0.001);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_osz_reports_variable_bpm_when_all_difficulties_do() {
        let dir = std::env::temp_dir().join(format!("osu-test-var-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let osz = dir.join("set.osz");
        let variable = BASIC_OSU.replace("1500,-100,4,2,0,60,0,0", "1500,250.0,4,2,0,60,1,0");
        write_test_osz(&osz, &[("map.osu", &variable)]);

        assert_eq!(read_osz_timings(&osz), Err(Skip::VariableBpm));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn extract_audio_finds_file_regardless_of_case_and_prefix() {
        let dir = std::env::temp_dir().join(format!("osu-test-ext-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let osz = dir.join("set.osz");
        write_test_osz(
            &osz,
            &[("map.osu", BASIC_OSU), ("AUDIO/Song.MP3", "fake mp3 bytes")],
        );

        let dest = extract_audio(&osz, "song.mp3", &dir).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "fake mp3 bytes");
        assert!(dest.extension().unwrap() == "MP3");

        std::fs::remove_dir_all(&dir).ok();
    }
}
