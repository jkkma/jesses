//! A verified lossless source gives scene detection, chunk encoding and quality
//! reference readers the same processed frames, including changed frame counts.
use super::{check_cancel, encode_plan::Plan, files, process_error};
use crate::supervisor::{self, CommandSpec, ProcessEvent};
use media_core::AppError;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    io::{BufRead, BufReader, Read},
    path::Path,
    time::Duration,
};
use tokio::sync::{mpsc, watch};

const TIME_LIMIT: Duration = Duration::from_secs(24 * 60 * 60);
fn error(path: &Path, detail: impl Into<String>) -> AppError {
    files::error("AV1AN_PREPARE_FAILED", detail, path)
}

pub(super) struct Prepared {
    pub video: files::Temporary,
    pub producer: CommandSpec,
}

impl Prepared {
    #[allow(clippy::too_many_arguments)]
    pub async fn build(
        source: &Path,
        output: &Path,
        id: &str,
        plan: &Plan,
        ffmpeg: &Path,
        trim: Option<(u32, u32)>,
        text: Option<&str>,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Self, AppError> {
        check_cancel(cancel)?;
        if plan.requires_qtgmc() {
            return Err(error(
                source,
                "QTGMC requires its separate verified frameserver preparation.",
            ));
        }
        let video = files::Temporary::create(output, &format!("{id}-processed-source"))?;
        let mut processing = plan.clone();
        if let Some((start, end)) = trim {
            processing.trim = Some(media_core::VideoTrim {
                start_frame: start,
                end_frame_exclusive: end,
                time: None,
            });
        }
        let mut args: Vec<OsString> = [
            "-hide_banner",
            "-nostdin",
            "-v",
            "warning",
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
        .collect();
        args.splice(0..0, plan.tone_map_device_args());
        args.push(source.as_os_str().to_owned());
        args.extend(["-map".into(), format!("0:{}", plan.video_index).into()]);
        let mut filters = processing
            .decoder_filter_with_text(text)
            .into_iter()
            .collect::<Vec<_>>();
        filters.push(format!("setsar={}", plan.output_sar().replace(':', "/")));
        args.extend(["-vf".into(), filters.join(",").into()]);
        args.extend([
            "-r".into(),
            format!("{}/{}", plan.fps_num, plan.fps_den).into(),
            "-fps_mode".into(),
            "cfr".into(),
            "-an".into(),
            "-sn".into(),
            "-dn".into(),
            "-map_metadata".into(),
            "-1".into(),
            "-map_chapters".into(),
            "-1".into(),
            "-pix_fmt".into(),
            plan.output_pixel_format.into(),
            "-color_primaries".into(),
            plan.primaries.to_string().into(),
            "-color_trc".into(),
            plan.transfer.to_string().into(),
            "-colorspace".into(),
            plan.matrix.to_string().into(),
            "-color_range".into(),
            if plan.full_range { "pc" } else { "tv" }.into(),
            "-chroma_sample_location".into(),
            plan.chroma.into(),
            "-c:v".into(),
            "ffv1".into(),
            "-level".into(),
            "3".into(),
            "-g".into(),
            "1".into(),
            "-slicecrc".into(),
            "1".into(),
            "-f".into(),
            "matroska".into(),
            "pipe:1".into(),
        ]);
        Ok(Self {
            video,
            producer: CommandSpec {
                executable: ffmpeg.to_owned(),
                args,
                cwd: None,
            },
        })
    }

    pub async fn run(
        &self,
        cancel: watch::Receiver<bool>,
        events: mpsc::Sender<ProcessEvent>,
        log: &Path,
    ) -> Result<(), AppError> {
        if let Some(parent) = log.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|cause| error(log, cause.to_string()))?;
        }
        // Only our reserved handle receives the bytes. FFmpeg never reopens a
        // pathname and cannot replace the file reservation.
        let mut output = self.video.clone_file()?;
        let result = supervisor::run_streaming_stdout(
            &self.producer,
            cancel,
            64 * 1024,
            TIME_LIMIT,
            move |reader| {
                let bytes = std::io::copy(reader, &mut output).map_err(|e| e.to_string())?;
                output.sync_all().map_err(|e| e.to_string())?;
                Ok(bytes)
            },
        )
        .await
        .map_err(|cause| process_error(cause, &self.video.path))?;
        let detail = format!(
            "Processed source: {} bytes.\n{}",
            result.value,
            String::from_utf8_lossy(&result.stderr)
        );
        // This is the same owned attempt-log namespace as the encoder logs.
        tokio::fs::write(log, detail.as_bytes())
            .await
            .map_err(|cause| error(log, cause.to_string()))?;
        let _ = events.try_send(ProcessEvent::Stderr(detail));
        if !result.status.success() {
            return Err(error(
                &self.video.path,
                "FFmpeg failed while preparing the lossless processed source. Review the preparation log.",
            ));
        }
        self.video.flush_nonempty_async().await
    }

    pub async fn validate(
        &self,
        ffprobe: &Path,
        plan: &Plan,
        expected_frames: usize,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        validate(ffprobe, &self.video.path, plan, expected_frames, cancel).await
    }

    pub async fn decoded_identity(
        &self,
        ffmpeg: &Path,
        expected_frames: usize,
        cancel: &watch::Receiver<bool>,
    ) -> Result<String, AppError> {
        decoded_identity(ffmpeg, &self.video.path, 0, expected_frames, cancel).await
    }
}

fn primaries(value: u8) -> &'static str {
    match value {
        1 => "bt709",
        5 => "bt470bg",
        6 => "smpte170m",
        9 => "bt2020",
        _ => "unknown",
    }
}
fn transfer(value: u8) -> &'static str {
    match value {
        1 => "bt709",
        5 => "bt470bg",
        6 => "smpte170m",
        16 => "smpte2084",
        18 => "arib-std-b67",
        _ => "unknown",
    }
}
fn matrix(value: u8) -> &'static str {
    match value {
        1 => "bt709",
        5 => "bt470bg",
        6 => "smpte170m",
        9 => "bt2020nc",
        _ => "unknown",
    }
}

#[derive(Deserialize)]
struct Frame {
    best_effort_timestamp_time: Option<String>,
    interlaced_frame: Option<u8>,
    width: Option<u32>,
    height: Option<u32>,
    pix_fmt: Option<String>,
    sample_aspect_ratio: Option<String>,
    color_space: Option<String>,
    color_transfer: Option<String>,
    color_primaries: Option<String>,
    color_range: Option<String>,
}

async fn validate(
    ffprobe: &Path,
    input: &Path,
    plan: &Plan,
    expected_frames: usize,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let document = super::probe(ffprobe, input, cancel, Some(&[0])).await?;
    let stream = document
        .streams
        .first()
        .ok_or_else(|| error(input, "The processed source has no video."))?;
    let rate_ok = stream
        .avg_frame_rate
        .as_deref()
        .and_then(|s| s.split_once('/'))
        .and_then(|(n, d)| Some((n.parse::<u64>().ok()?, d.parse::<u64>().ok()?)))
        .is_some_and(|(n, d)| {
            n > 0 && d > 0 && n * u64::from(plan.fps_den) == d * u64::from(plan.fps_num)
        });
    if expected_frames == 0
        || document.streams.len() != 1
        || stream.codec_name.as_deref() != Some("ffv1")
        || stream.width != Some(plan.width)
        || stream.height != Some(plan.height)
        || !plan.matches_output_format(stream.pix_fmt.as_deref())
        || stream.sample_aspect_ratio.as_deref() != Some(plan.output_sar())
        || !rate_ok
        || stream.color_primaries.as_deref() != Some(primaries(plan.primaries))
        || stream.color_transfer.as_deref() != Some(transfer(plan.transfer))
        || stream.color_space.as_deref() != Some(matrix(plan.matrix))
        || stream.color_range.as_deref() != Some(if plan.full_range { "pc" } else { "tv" })
    {
        return Err(error(
            input,
            "The FFV1 source codec, geometry, color, pixel aspect ratio or frame rate differs from its validated plan.",
        ));
    }
    let mut args: Vec<OsString> = ["-v", "error", "-err_detect", "explode", "-select_streams", "0", "-show_frames", "-show_entries",
        "frame=best_effort_timestamp_time,interlaced_frame,width,height,pix_fmt,sample_aspect_ratio,color_space,color_transfer,color_primaries,color_range", "-of", "json", "-i"]
        .into_iter().map(OsString::from).collect();
    args.push(input.as_os_str().to_owned());
    let plan = plan.clone();
    let result = supervisor::run_streaming_stdout(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        TIME_LIMIT,
        move |reader| {
            let mut count = 0usize;
            let mut failure = None;
            super::encode::frame_scan::parse::<Frame>(reader, |frame| {
                if failure.is_some() {
                    return;
                }
                let time = frame
                    .best_effort_timestamp_time
                    .as_deref()
                    .and_then(|s| s.parse::<f64>().ok());
                if count >= expected_frames
                    || !time.is_some_and(|t| {
                        t.is_finite()
                            && (t - count as f64 * plan.frame_seconds()).abs() <= 0.001_002
                    })
                    || frame.interlaced_frame != Some(0)
                    || frame.width != Some(plan.width)
                    || frame.height != Some(plan.height)
                    || !plan.matches_output_format(frame.pix_fmt.as_deref())
                    || frame.sample_aspect_ratio.as_deref() != Some(plan.output_sar())
                    || frame.color_primaries.as_deref() != Some(primaries(plan.primaries))
                    || frame.color_transfer.as_deref() != Some(transfer(plan.transfer))
                    || frame.color_space.as_deref() != Some(matrix(plan.matrix))
                    || frame.color_range.as_deref()
                        != Some(if plan.full_range { "pc" } else { "tv" })
                {
                    failure = Some(format!(
                        "Processed frame {count} has unexpected timing, scan, geometry or color."
                    ));
                }
                count += 1;
            })?;
            if let Some(failure) = failure {
                return Err(failure);
            }
            if count != expected_frames {
                return Err(format!(
                    "Processed source decoded {count} frames; expected {expected_frames}."
                ));
            }
            Ok(())
        },
    )
    .await
    .map_err(|cause| process_error(cause, input))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(error(
            input,
            "The processed source failed complete frame decoding.",
        ));
    }
    check_cancel(cancel)
}

fn checksum(reader: &mut dyn Read, expected_frames: usize) -> Result<String, String> {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::with_capacity(1024);
    let mut hash = Sha256::new();
    let mut count = 0usize;
    let mut next_pts = 0i64;
    let mut header = false;
    let mut time_base = false;
    loop {
        line.clear();
        let read = reader
            .by_ref()
            .take(1025)
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        if read > 1024 || line.last() != Some(&b'\n') {
            return Err("Oversized or incomplete frame fingerprint record.".into());
        }
        let line = std::str::from_utf8(&line)
            .map_err(|e| e.to_string())?
            .trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('#') {
            if count != 0 {
                return Err("Frame fingerprint header appears after frame data.".into());
            }
            header |= line == "#hash: SHA256";
            time_base |= line.starts_with("#tb 0: ");
            if !line.starts_with("#software:") {
                hash.update(line.as_bytes());
                hash.update(b"\n");
            }
            continue;
        }
        let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
        if !header
            || !time_base
            || fields.len() != 6
            || fields[0] != "0"
            || count >= expected_frames
        {
            return Err("Unexpected decoded frame fingerprint layout or count.".into());
        }
        let dts = fields[1].parse::<i64>().map_err(|e| e.to_string())?;
        let pts = fields[2].parse::<i64>().map_err(|e| e.to_string())?;
        let duration = fields[3].parse::<i64>().map_err(|e| e.to_string())?;
        let size = fields[4].parse::<u64>().map_err(|e| e.to_string())?;
        if dts != pts
            || pts != next_pts
            || duration <= 0
            || size == 0
            || fields[5].len() != 64
            || !fields[5].bytes().all(|v| v.is_ascii_hexdigit())
        {
            return Err(
                "Decoded frame fingerprint has invalid timing or a malformed digest.".into(),
            );
        }
        next_pts = pts
            .checked_add(duration)
            .ok_or("Frame timestamp overflow.")?;
        hash.update(
            format!(
                "{pts},{duration},{size},{}\n",
                fields[5].to_ascii_lowercase()
            )
            .as_bytes(),
        );
        count += 1;
    }
    if count == 0 || count != expected_frames {
        return Err("Decoded frame fingerprint has incomplete frame coverage.".into());
    }
    Ok(format!("sha256:{:x};frames:{count}", hash.finalize()))
}

pub(super) async fn decoded_identity(
    ffmpeg: &Path,
    input: &Path,
    stream_index: u32,
    expected_frames: usize,
    cancel: &watch::Receiver<bool>,
) -> Result<String, AppError> {
    let mut args: Vec<OsString> = [
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
    .collect();
    args.push(input.as_os_str().to_owned());
    args.extend(["-map".into(), format!("0:{stream_index}").into()]);
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
        TIME_LIMIT,
        move |reader| checksum(reader, expected_frames),
    )
    .await
    .map_err(|cause| process_error(cause, input))?;
    if !result.status.success() || !result.stderr.is_empty() {
        return Err(error(
            input,
            "Could not verify every decoded processed frame.",
        ));
    }
    check_cancel(cancel)?;
    Ok(result.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn decoded_pixel_digests(
        ffmpeg: &Path,
        input: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Vec<String> {
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
        args.extend(
            [
                "-map",
                "0:0",
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
        let result = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg.to_owned(),
                args,
                cwd: None,
            },
            cancel.clone(),
            256 * 1024,
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        assert!(
            result.status.success() && result.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout)
            .unwrap()
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .map(|line| {
                let fields = line.split(',').map(str::trim).collect::<Vec<_>>();
                assert_eq!(fields.len(), 6, "unexpected framemd5 row: {line}");
                assert_eq!(fields[0], "0", "unexpected stream in framemd5 row");
                assert_eq!(fields[5].len(), 64, "unexpected SHA-256 digest");
                fields[5].to_ascii_lowercase()
            })
            .collect()
    }

    fn record(pts: i32, pixel: &str) -> String {
        format!("0, {pts}, {pts}, 1, 48, {pixel}\n")
    }
    fn input(records: &str) -> String {
        format!("#hash: SHA256\n#tb 0: 1/24\n{records}")
    }
    #[test]
    fn fingerprints_bind_pixels_and_complete_timeline() {
        let a = "a".repeat(64);
        let b = "b".repeat(64);
        let valid = input(&(record(0, &a) + &record(1, &b)));
        let fingerprint = checksum(&mut valid.as_bytes(), 2).unwrap();
        let changed = input(&(record(0, &b) + &record(1, &b)));
        assert_ne!(fingerprint, checksum(&mut changed.as_bytes(), 2).unwrap());
        for invalid in [
            input(&record(0, &a)),
            input(&(record(0, &a) + &record(2, &b))),
            valid.trim_end().to_owned(),
            format!("{}\n", "x".repeat(2048)),
        ] {
            assert!(checksum(&mut invalid.as_bytes(), 2).is_err());
        }
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg and FFprobe"]
    async fn actual_preparation_has_exact_trim_rate_aspect_and_stable_decoded_identity() {
        let root = std::env::temp_dir().join(format!(
            "jesses-prepared-source-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let (_sender, cancel) = watch::channel(false);
        let ffmpeg = super::super::discover("ffmpeg", &cancel).await.unwrap();
        let ffprobe = super::super::discover("ffprobe", &cancel).await.unwrap();
        let input = root.join("source.mkv");
        let mut args = ["-v", "error", "-nostdin", "-f", "lavfi", "-i", "testsrc2=s=192x112:r=24:d=2", "-vf",
            "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709,format=yuv420p10le",
            "-c:v", "ffv1", "-level", "3", "-pix_fmt", "yuv420p10le", "-chroma_sample_location", "left", "-n"].into_iter().map(OsString::from).collect::<Vec<_>>();
        args.push(input.as_os_str().to_owned());
        let result = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg.clone(),
                args,
                cwd: None,
            },
            cancel.clone(),
            64 * 1024,
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let original = Sha256::digest(std::fs::read(&input).unwrap());
        let document = super::super::probe(&ffprobe, &input, &cancel, Some(&[0]))
            .await
            .unwrap();
        let settings = media_core::EncodeSettings {
            backend: media_core::EncodeBackend::Av1an,
            temporal: Some(media_core::TemporalSettings {
                frame_rate: Some(media_core::FrameRate {
                    numerator: 12,
                    denominator: 1,
                }),
                aspect_ratio: Some(media_core::AspectRatioSettings {
                    kind: media_core::AspectRatioKind::Display,
                    numerator: 4,
                    denominator: 3,
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut plan =
            Plan::build(&document, &document.selected(&[0]).unwrap(), &settings).unwrap();
        assert_eq!(plan.activate_temporal(24).unwrap(), 12);
        assert_eq!(plan.output_pixel_format, "yuv420p10le");
        let source_pixels = decoded_pixel_digests(&ffmpeg, &input, &cancel).await;
        assert_eq!(source_pixels.len(), 48);
        assert_eq!(
            source_pixels
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len(),
            source_pixels.len(),
            "the oracle source must identify every frame uniquely"
        );
        let expected_pixels = (6..30)
            .step_by(2)
            .map(|index| source_pixels[index].clone())
            .collect::<Vec<_>>();
        let output = root.join("eventual-output.mkv");
        let mut previous = None;
        for attempt in ["first", "second"] {
            let prepared = Prepared::build(
                &input,
                &output,
                attempt,
                &plan,
                &ffmpeg,
                Some((6, 30)),
                None,
                &cancel,
            )
            .await
            .unwrap();
            let (events, mut receiver) = mpsc::channel(16);
            let observer = tokio::spawn(async move { while receiver.recv().await.is_some() {} });
            prepared
                .run(
                    cancel.clone(),
                    events,
                    &root.join("new-logs").join(format!("{attempt}.log")),
                )
                .await
                .unwrap();
            observer.await.unwrap();
            prepared
                .validate(&ffprobe, &plan, 12, &cancel)
                .await
                .unwrap();
            assert!(
                prepared
                    .validate(&ffprobe, &plan, 11, &cancel)
                    .await
                    .is_err()
            );
            let fingerprint = prepared
                .decoded_identity(&ffmpeg, 12, &cancel)
                .await
                .unwrap();
            assert_eq!(
                decoded_pixel_digests(&ffmpeg, &prepared.video.path, &cancel).await,
                expected_pixels,
                "trim and 24-to-12 fps conversion must retain source frames 6, 8, ... 28"
            );
            if let Some(previous) = previous {
                assert_eq!(fingerprint, previous);
            }
            previous = Some(fingerprint);
        }
        assert_eq!(original, Sha256::digest(std::fs::read(&input).unwrap()));
        assert!(!output.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
