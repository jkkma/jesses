//! Exact decoded-frame characterization for progressive captures padded with
//! duplicate frames. The scan retains one SHA-256 digest and bounded counters;
//! source pixels and full per-frame output never enter application memory.
use super::{check_cancel, files, process_error};
use crate::supervisor::{self, CommandSpec, SupervisorError};
use media_core::AppError;
use std::{
    collections::BTreeSet,
    ffi::OsString,
    io::{BufRead, BufReader, Read},
    path::Path,
    time::Duration,
};
use tokio::sync::watch;

const MAX_LINE: usize = 1_024;
const MAX_FRAMES: usize = 100_000_000;

fn error(message: impl Into<String>, path: &Path) -> AppError {
    files::error("CADENCE_REPAIR_UNSUPPORTED", message, path)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Report {
    pub source_frames: usize,
    pub unique_frames: usize,
    pub duplicate_frames: usize,
    pub duplicate_runs: usize,
    pub minimum_run: usize,
    pub maximum_run: usize,
    pub run_lengths: Vec<usize>,
    pub repeat_length_transitions: usize,
}

impl Report {
    pub fn summary(&self) -> String {
        let lengths = self
            .run_lengths
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "Exact duplicate cadence: {} decoded frames, {} unique, {} duplicates across {} repeated runs; run lengths {}–{} (observed {{{lengths}}}, {} adjacent repeat-length transitions).",
            self.source_frames,
            self.unique_frames,
            self.duplicate_frames,
            self.duplicate_runs,
            self.minimum_run,
            self.maximum_run,
            self.repeat_length_transitions,
        )
    }

    fn guarded(self, expected_frames: usize, path: &Path) -> Result<Self, AppError> {
        if self.source_frames != expected_frames {
            return Err(error(
                format!(
                    "The exact-frame cadence scan returned {} frames after the selected interval; {} validated frames were expected.",
                    self.source_frames, expected_frames
                ),
                path,
            ));
        }
        if self.source_frames < 8
            || self.unique_frames < 4
            || self.duplicate_frames < 2
            || self.duplicate_runs < 2
            || self.maximum_run > 8
            || self.duplicate_frames * 5 > self.source_frames * 4
        {
            return Err(error(
                format!(
                    "The decoded frames do not form a guarded padded-capture pattern. {}",
                    self.summary()
                ),
                path,
            ));
        }
        Ok(self)
    }
}

#[derive(Default)]
struct Characterizer {
    frames: usize,
    unique: usize,
    duplicates: usize,
    duplicate_runs: usize,
    previous_hash: Option<String>,
    previous_pts: Option<i64>,
    previous_duration: Option<i64>,
    current_run: usize,
    previous_run: Option<usize>,
    minimum_run: usize,
    maximum_run: usize,
    run_lengths: BTreeSet<usize>,
    changes: usize,
}

impl Characterizer {
    fn finish_run(&mut self) {
        if self.current_run == 0 {
            return;
        }
        self.minimum_run = if self.minimum_run == 0 {
            self.current_run
        } else {
            self.minimum_run.min(self.current_run)
        };
        self.maximum_run = self.maximum_run.max(self.current_run);
        self.run_lengths.insert(self.current_run);
        if self.current_run > 1 {
            self.duplicate_runs += 1;
        }
        if self
            .previous_run
            .is_some_and(|previous| previous != self.current_run)
        {
            self.changes += 1;
        }
        self.previous_run = Some(self.current_run);
        self.current_run = 0;
    }

    fn push(&mut self, pts: i64, duration: i64, hash: &str) -> Result<(), String> {
        if self.frames >= MAX_FRAMES {
            return Err("The cadence scan exceeds its frame limit.".into());
        }
        if duration <= 0
            || self.previous_pts.is_some_and(|previous| {
                pts != previous + self.previous_duration.expect("previous duration")
            })
        {
            return Err("The cadence checksum timeline is not contiguous CFR.".into());
        }
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("The cadence scan returned an invalid SHA-256 digest.".into());
        }
        if self.previous_hash.as_deref() == Some(hash) {
            self.duplicates += 1;
            self.current_run += 1;
        } else {
            self.finish_run();
            self.unique += 1;
            self.current_run = 1;
            self.previous_hash = Some(hash.to_owned());
        }
        self.previous_pts = Some(pts);
        self.previous_duration = Some(duration);
        self.frames += 1;
        Ok(())
    }

    fn finish(mut self) -> Result<Report, String> {
        self.finish_run();
        if self.frames == 0 {
            return Err("The cadence scan returned no decoded frames.".into());
        }
        Ok(Report {
            source_frames: self.frames,
            unique_frames: self.unique,
            duplicate_frames: self.duplicates,
            duplicate_runs: self.duplicate_runs,
            minimum_run: self.minimum_run,
            maximum_run: self.maximum_run,
            run_lengths: self.run_lengths.into_iter().collect(),
            repeat_length_transitions: self.changes,
        })
    }
}

fn parse(reader: &mut dyn Read) -> Result<Report, String> {
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    let mut hash_header = false;
    let mut columns = false;
    let mut characterizer = Characterizer::default();
    loop {
        line.clear();
        let count = reader
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        if count == 0 {
            break;
        }
        if count > MAX_LINE || !line.ends_with('\n') {
            return Err("The cadence scan returned an oversized or truncated record.".into());
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            hash_header |= line == "#hash: SHA256";
            columns |= line == "#stream#, dts,        pts, duration,     size, hash";
            continue;
        }
        if !hash_header || !columns {
            return Err("The cadence scan omitted its SHA-256 framemd5 header.".into());
        }
        let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
        if fields.len() != 6 || fields[0] != "0" {
            return Err("The cadence scan returned an invalid frame record.".into());
        }
        let dts = fields[1]
            .parse::<i64>()
            .map_err(|_| "The cadence scan returned an invalid DTS.".to_owned())?;
        let pts = fields[2]
            .parse::<i64>()
            .map_err(|_| "The cadence scan returned an invalid PTS.".to_owned())?;
        let duration = fields[3]
            .parse::<i64>()
            .map_err(|_| "The cadence scan returned an invalid duration.".to_owned())?;
        let size = fields[4]
            .parse::<u64>()
            .map_err(|_| "The cadence scan returned an invalid frame size.".to_owned())?;
        if dts != pts || size == 0 {
            return Err("The cadence scan returned reordered or empty raw frames.".into());
        }
        characterizer.push(pts, duration, fields[5])?;
    }
    characterizer.finish()
}

pub(super) async fn scan(
    ffmpeg: &Path,
    input: &Path,
    stream_index: u32,
    interval: Option<(usize, usize)>,
    expected_frames: usize,
    cancel: &watch::Receiver<bool>,
) -> Result<Report, AppError> {
    check_cancel(cancel)?;
    let mut args = [
        "-v",
        "error",
        "-nostdin",
        "-xerror",
        "-err_detect",
        "explode",
        "-noautorotate",
        "-protocol_whitelist",
        "file",
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .collect::<Vec<_>>();
    args.push(input.as_os_str().to_owned());
    args.extend(["-map".into(), format!("0:{stream_index}").into()]);
    if let Some((start, end)) = interval {
        args.extend([
            "-vf".into(),
            format!("trim=start_frame={start}:end_frame={end},setpts=PTS-STARTPTS").into(),
        ]);
    }
    args.extend(
        [
            "-an",
            "-sn",
            "-dn",
            "-fps_mode",
            "passthrough",
            "-f",
            "framemd5",
            "-hash",
            "sha256",
            "pipe:1",
        ]
        .into_iter()
        .map(OsString::from),
    );
    let result = supervisor::run_streaming_stdout(
        &CommandSpec {
            executable: ffmpeg.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(24 * 60 * 60),
        parse,
    )
    .await
    .map_err(|error| match error {
        SupervisorError::OutputParse(detail) => files::error(
            "CADENCE_SCAN_FAILED",
            format!("The exact decoded-frame cadence report was invalid: {detail}"),
            input,
        ),
        error => process_error(error, input),
    })?;
    check_cancel(cancel)?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(files::error(
            "CADENCE_SCAN_FAILED",
            format!(
                "The exact decoded-frame cadence scan failed: {}",
                String::from_utf8_lossy(&result.stderr)
            ),
            input,
        ));
    }
    result.value.guarded(expected_frames, input)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stream(hashes: &[&str]) -> Vec<u8> {
        let mut value = "#format: frame checksums\n#version: 2\n#hash: SHA256\n#software: Lavf63.1.101\n#tb 0: 1/60\n#media_type 0: video\n#codec_id 0: rawvideo\n#dimensions 0: 64x64\n#sar 0: 1/1\n#stream#, dts,        pts, duration,     size, hash\n".to_owned();
        for (index, hash) in hashes.iter().enumerate() {
            value.push_str(&format!("0, {index}, {index}, 1, 6144, {hash}\n"));
        }
        value.into_bytes()
    }

    #[test]
    fn changing_exact_duplicate_runs_are_characterized_with_bounded_state() {
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let c = "c".repeat(64);
        let d = "d".repeat(64);
        let hashes = [
            a.as_str(),
            a.as_str(),
            b.as_str(),
            b.as_str(),
            b.as_str(),
            c.as_str(),
            c.as_str(),
            d.as_str(),
            d.as_str(),
            d.as_str(),
        ];
        let report = parse(&mut stream(&hashes).as_slice()).unwrap();
        assert_eq!(report.source_frames, 10);
        assert_eq!(report.unique_frames, 4);
        assert_eq!(report.duplicate_frames, 6);
        assert_eq!(report.duplicate_runs, 4);
        assert_eq!(report.run_lengths, [2, 3]);
        assert_eq!(report.repeat_length_transitions, 3);
        assert!(report.guarded(10, Path::new("capture.mkv")).is_ok());
    }

    #[test]
    fn malformed_timestamps_hashes_and_unpadded_sources_are_rejected() {
        let hash = "e".repeat(64);
        let mut malformed = stream(&[&hash, &hash]);
        let position = malformed
            .windows(7)
            .position(|window| window == b"0, 1, 1")
            .unwrap();
        malformed[position + 6] = b'2';
        assert!(parse(&mut malformed.as_slice()).is_err());

        let hashes = (0..8)
            .map(|index| format!("{index:064x}"))
            .collect::<Vec<_>>();
        let refs = hashes.iter().map(String::as_str).collect::<Vec<_>>();
        let report = parse(&mut stream(&refs).as_slice()).unwrap();
        assert!(report.guarded(8, Path::new("capture.mkv")).is_err());
    }
}
