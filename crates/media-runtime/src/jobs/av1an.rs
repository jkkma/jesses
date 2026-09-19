//! Chunked SVT encoding under the same process ownership and artifact gates.
use super::{encode::encoder_parameters, encode_plan::Plan, *};

mod launcher;
mod metrics;
mod options;
pub(super) use options::validate_settings;
mod recovery;
mod recovery_receipts;
pub(super) use recovery::{PreparedSource, Recovery};

pub(super) fn validate_encoder_path(encoder: &Path) -> Result<(), AppError> {
    launcher::validate_encoder_path(encoder)
}

pub(super) fn validate_input(document: &Document, plan: &Plan) -> Result<(), AppError> {
    if document
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"))
        .is_none_or(|s| s.index != plan.video_index)
    {
        return Err(AppError::new(
            "ENCODE_INPUT_UNSUPPORTED",
            "av1an currently encodes the first video track. Choose that track or use standalone SVT-AV1.",
            None,
        ));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn arguments(
    input: &Path,
    output: &Path,
    work: &Path,
    log: &Path,
    plan: &Plan,
    settings: &EncodeSettings,
    resume: bool,
    source_filter: Option<&str>,
) -> Vec<OsString> {
    // Only planner-owned scalars enter av1an's nested encoder argument string.
    // User paths are always separate native arguments, never shell text.
    let params = encoder_parameters(plan, settings)
        .iter()
        .map(|v| v.to_str().expect("ASCII encoder parameters"))
        .collect::<Vec<_>>()
        .join(" ");
    let mut args: Vec<OsString> = [
        "--encoder",
        "svt-av1",
        "--passes",
        "1",
        "--no-defaults",
        "--concat",
        // av1an's av-ivf concatenator can panic on valid small AV1 packets.
        // FFmpeg copies the IVF chunks; our complete record and decode checks
        // still verify the resulting stream before publication.
        "ffmpeg",
        "--pix-format",
        "yuv420p10le",
        "--cache-mode",
        "temp",
        "--max-tries",
        "3",
        "--keep",
        "--verbose",
        "--log-level",
        if settings
            .av1an_options
            .is_some_and(|options| options.target_quality.is_some())
        {
            "debug"
        } else {
            "info"
        },
        "-y",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    options::append(&mut args, settings, &params);
    if resume {
        args.push("--resume".into());
    }
    if let Some(filter) = source_filter {
        // The generated filter contains only planner-owned labels, integers,
        // and fixed FFmpeg options. Paths never enter av1an's nested parser.
        args.extend(["--ffmpeg".into(), format!("-vf {filter}").into()]);
        if settings
            .av1an_options
            .is_some_and(|options| options.target_quality.is_some())
        {
            // Target probes encode through --ffmpeg. Score the reference after
            // the same deterministic processing so the search and final chunk
            // compare corresponding pixels, timing, color and geometry.
            args.extend(["--vmaf-filter".into(), filter.into()]);
        }
    }
    for (key, value) in [
        ("-i", input.as_os_str().to_owned()),
        ("-o", output.as_os_str().to_owned()),
        ("--temp", work.join("chunks").into_os_string()),
        ("--log-file", log.as_os_str().to_owned()),
        ("--workers", settings.workers.to_string().into()),
        ("--video-params", params.into()),
        // av1an starts with `-map 0`; disabling audio/subtitles/data still
        // leaves attachments, which its concat step mistakes for valid audio.
        ("--audio-params", "-an -sn -dn -map -0:t?".into()),
    ] {
        args.extend([key.into(), value]);
    }
    args
}

impl JobManager {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn encode_av1an(
        &self,
        id: &str,
        input: &Path,
        encoder: &Path,
        executable: &Path,
        ffmpeg: &Path,
        ffprobe: &Path,
        recovery: &Recovery,
        ivf: &Temporary,
        plan: &Plan,
        settings: &EncodeSettings,
        cancel: &watch::Receiver<bool>,
        log_path: &Path,
        frame_count: usize,
    ) -> Result<(), AppError> {
        validate_encoder_path(encoder)?;
        if plan.is_hdr10()
            && settings
                .av1an_options
                .is_some_and(|options| options.target_quality.is_some())
        {
            return Err(AppError::new(
                "ENCODE_INPUT_UNSUPPORTED",
                "Quality targeting currently requires SDR input. These metric pipelines are not qualified for preserved HDR output.",
                None,
            ));
        }
        let work = recovery.root.clone();
        let source_filter = recovery.source_filter()?;
        // A crashed staged host remains in its owned attempt directory. Never
        // replace it or let it shadow the encoder selected for this attempt.
        let launch_directory = work.join(format!(
            "launch-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        tokio::fs::create_dir(&launch_directory)
            .await
            .map_err(|e| {
                files::error(
                    "OUTPUT_CREATE_FAILED",
                    format!("Cannot reserve av1an workspace: {e}"),
                    &work,
                )
            })?;
        let launch_executable = executable.to_owned();
        let launch_encoder = encoder.to_owned();
        let launch_work = launch_directory.clone();
        let launch_ffmpeg = ffmpeg.to_owned();
        let launch_ffprobe = ffprobe.to_owned();
        let mut launch = tokio::task::spawn_blocking(move || {
            launcher::Launch::prepare(
                &launch_executable,
                &launch_encoder,
                &launch_ffmpeg,
                &launch_ffprobe,
                &launch_work,
            )
        })
        .await
        .map_err(|e| files::error("AV1AN_STAGE_FAILED", e.to_string(), executable))??;
        check_cancel(cancel)?;
        let version = supervisor::run_capture_with_environment(
            &CommandSpec {
                executable: launch.executable.clone(),
                args: vec!["--version".into()],
                cwd: Some(work.clone()),
            },
            cancel.clone(),
            128 * 1024,
            Duration::from_secs(15),
            Some(&launch.environment),
        )
        .await
        .map_err(|e| process_error(e, executable))?;
        if !version.status.success() {
            return Err(files::error(
                "TOOL_FAILED",
                "av1an failed its version check.",
                executable,
            ));
        }
        let version_text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&version.stdout),
            String::from_utf8_lossy(&version.stderr)
        );
        let configured = settings.av1an_options.unwrap_or_default();
        options::validate_plugin(&version_text, configured, source_filter.is_some())
            .map_err(|message| files::error("AV1AN_DEPENDENCY_MISSING", message, executable))?;
        options::capabilities(
            &launch.executable,
            ffmpeg,
            &launch.environment,
            &work,
            settings,
            cancel,
        )
        .await?;
        let version = version_text
            .lines()
            .find(|v| !v.is_empty())
            .unwrap_or("unknown av1an version")
            .to_owned();
        recovery.version(version_text).await?;
        recovery.verify_segments(ffmpeg, input, cancel).await?;
        let internal_log = log_path.with_extension("av1an.log");
        self.change(id, |snapshot| {
            append_log(snapshot, format!("Tool: {} — {version}", executable.display()));
            append_log(snapshot, format!("av1an selected encoder: {}", encoder.display()));
            append_log(snapshot, format!("av1an: {} parallel workers; {} source chunks, {:?} splitting, maximum {} frames (0 means unlimited). Workspace: {}", settings.workers, options::chunk_method(configured), configured.split_method, configured.maximum_chunk_frames, work.display()));
            if let Some(target) = configured.target_quality {
                append_log(snapshot, format!("{} target {:.1}–{:.1}, mean score at {}×{}, CRF {}–{}, at most {} probes, sampling every {} frame(s). A search may finish outside the requested score range when its bounds/probes are exhausted.", metrics::label(target.metric), f64::from(target.minimum_score_tenths)/10.0, f64::from(target.maximum_score_tenths)/10.0, target.probe_width, target.probe_height, target.minimum_crf, target.maximum_crf, target.probes, target.probing_rate));
                if source_filter.is_some() { append_log(snapshot, "Quality target: probe encodes and reference scoring use the same validated trim, temporal, tone-map, framing, and aspect-ratio filter chain as final chunks.".into()); }
                else { append_log(snapshot, "Quality target: probe encodes and reference scoring read the same verified lossless processed source as final chunks.".into()); }
                if target.probing_rate > 1 { append_log(snapshot, "Quality target warning: sampled-frame scores may differ from scoring every frame; the target is not a full-output quality measurement.".into()); }
            }
            append_log(snapshot, format!("av1an detail log: {}", internal_log.display()));
            snapshot.log_path = Some(log_path.to_string_lossy().into_owned());
        }).await;
        let (sender, mut events) = mpsc::channel(256);
        let observer = self.clone();
        let event_id = id.to_owned();
        let event_recovery = recovery.clone();
        let progress_path = work.join("chunks/done.json");
        let frame_seconds = plan.frame_seconds();
        let event_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            let mut previous = 0;
            loop {
                tokio::select! {
                    event = events.recv() => {
                        let Some(ProcessEvent::Stdout(line) | ProcessEvent::Stderr(line)) = event else { break };
                        observer.change(&event_id, |snapshot| append_log(snapshot, line)).await;
                    }
                    _ = interval.tick() => {
                        if let Ok(summary) = event_recovery.checkpoint(false).await {
                            observer.change(&event_id, |snapshot| snapshot.recovery = Some(summary)).await;
                        }
                        if let Some((frames, chunks)) = read_progress(&progress_path, frame_count).await
                            && frames > previous {
                                previous = frames;
                                observer.change(&event_id, |snapshot| {
                                    snapshot.progress_seconds = Some(frames as f64 * frame_seconds);
                                    append_log(snapshot, format!("av1an: {chunks} chunks completed; {frames}/{frame_count} frames."));
                                }).await;
                        }
                    }
                }
            }
        });
        let pause = self
            .state
            .lock()
            .await
            .entries
            .iter()
            .find(|entry| entry.snapshot.id == id)
            .expect("registered av1an job")
            .pause
            .clone();
        let result = supervisor::run_with_environment(
            &CommandSpec {
                executable: launch.executable.clone(),
                args: arguments(
                    input,
                    &ivf.path,
                    &work,
                    &internal_log,
                    plan,
                    settings,
                    recovery.resume_chunks,
                    source_filter.as_deref(),
                ),
                cwd: Some(work.clone()),
            },
            cancel.clone(),
            sender,
            log_path,
            Duration::from_secs(24 * 60 * 60),
            Some(&launch.environment),
            Some(&pause),
        )
        .await;
        let _ = event_task.await;
        let stage_cleanup = launch.cleanup();
        let _ = tokio::fs::remove_dir(&launch_directory).await;
        // The process has exited, so seal its last complete receipts even when
        // cancellation won. A crash before this point reuses only prior receipts.
        let checkpoint = recovery
            .checkpoint(result.as_ref().is_ok_and(|result| result.status.success()))
            .await;
        match checkpoint {
            Ok(summary) => {
                self.change(id, |snapshot| snapshot.recovery = Some(summary))
                    .await
            }
            Err(error) => return Err(error),
        }
        let result = result.map_err(|e| process_error(e, input))?;
        stage_cleanup?;
        if !result.status.success() {
            return Err(files::error(
                "AV1AN_FAILED",
                format!(
                    "av1an failed ({}). See the job log for the failed chunk or missing dependency.",
                    result.status
                ),
                input,
            ));
        }
        if configured.chunk_method == media_core::Av1anChunkMethod::Hybrid {
            self.change(id, |snapshot| append_log(snapshot, "Verifying the complete decoded hybrid segment sequence against the original source.".into())).await;
        }
        recovery.verify_segments(ffmpeg, input, cancel).await?;
        ivf.flush_nonempty_async().await?;
        let mut file = ivf.clone_file()?;
        let geometry = (plan.width, plan.height);
        let rate = (plan.fps_num, plan.fps_den);
        let canceled = cancel.clone();
        let normalized = tokio::task::spawn_blocking(move || {
            normalize_ivf(&mut file, geometry, rate, frame_count, &canceled)
        })
        .await;
        check_cancel(cancel)?;
        let changed = normalized
            .map_err(|e| files::error("ENCODE_VALIDATION_FAILED", e.to_string(), &ivf.path))?
            .map_err(|e| files::error("ENCODE_VALIDATION_FAILED", e.to_string(), &ivf.path))?;
        if changed {
            self.change(id, |snapshot| append_log(snapshot, format!("Corrected av1an IVF rate to {}/{} after checking all {frame_count} sequential frame records.", rate.0, rate.1))).await;
        }
        check_cancel(cancel)
    }
}

#[derive(serde::Deserialize)]
struct Completed {
    frames: u64,
}
#[derive(serde::Deserialize)]
struct Progress {
    frames: u64,
    done: std::collections::BTreeMap<String, Completed>,
}

fn progress(bytes: &[u8], expected: usize) -> Option<(u64, usize)> {
    let state: Progress = serde_json::from_slice(bytes).ok()?;
    if state.frames != expected as u64 {
        return None;
    }
    let count = state
        .done
        .values()
        .try_fold(0u64, |sum, chunk| sum.checked_add(chunk.frames))?;
    (count <= state.frames).then_some((count, state.done.len()))
}

async fn read_progress(path: &Path, expected: usize) -> Option<(u64, usize)> {
    use tokio::io::AsyncReadExt;
    let file = tokio::fs::File::open(path).await.ok()?;
    let mut bytes = Vec::new();
    file.take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .await
        .ok()?;
    if bytes.len() > 4 * 1024 * 1024 {
        return None;
    }
    progress(&bytes, expected)
}

/// av1an's experimental IVF concatenator can write a fixed 30 fps header.
/// Correct only a structurally verified owned AV1 stream whose frame records
/// have the exact sequential timestamps/count already established by preflight.
fn normalize_ivf(
    file: &mut std::fs::File,
    geometry: (u32, u32),
    rate: (u32, u32),
    expected: usize,
    cancel: &watch::Receiver<bool>,
) -> std::io::Result<bool> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let invalid = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "av1an IVF frame structure does not match the validated source",
        )
    };
    file.seek(SeekFrom::Start(0))?;
    let length = file.metadata()?.len();
    let mut header = [0u8; 32];
    file.read_exact(&mut header)?;
    let u16_at = |offset| u16::from_le_bytes(header[offset..offset + 2].try_into().unwrap());
    if &header[..4] != b"DKIF"
        || u16_at(4) != 0
        || u16_at(6) != 32
        || &header[8..12] != b"AV01"
        || (u32::from(u16_at(12)), u32::from(u16_at(14))) != geometry
    {
        return Err(invalid());
    }
    let mut offset = 32u64;
    for index in 0..expected {
        if *cancel.borrow() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "Job canceled during IVF verification",
            ));
        }
        let mut frame = [0u8; 12];
        file.read_exact(&mut frame)?;
        let size = u32::from_le_bytes(frame[..4].try_into().unwrap());
        let timestamp = u64::from_le_bytes(frame[4..].try_into().unwrap());
        offset = offset
            .checked_add(12 + u64::from(size))
            .ok_or_else(invalid)?;
        if size == 0 || timestamp != index as u64 || offset > length {
            return Err(invalid());
        }
        file.seek(SeekFrom::Start(offset))?;
    }
    if offset != length {
        return Err(invalid());
    }
    let frame_count = u32::try_from(expected).map_err(|_| invalid())?;
    let changed = header[16..20] != rate.0.to_le_bytes()
        || header[20..24] != rate.1.to_le_bytes()
        || header[24..28] != frame_count.to_le_bytes();
    if changed {
        file.seek(SeekFrom::Start(16))?;
        file.write_all(&rate.0.to_le_bytes())?;
        file.write_all(&rate.1.to_le_bytes())?;
        file.write_all(&frame_count.to_le_bytes())?;
        file.sync_all()?;
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_plan(index: u32) -> (Document, Plan) {
        let video = |index| serde_json::json!({"index":index,"codec_type":"video","width":128,"height":96,"pix_fmt":"yuv420p","sample_aspect_ratio":"1:1","avg_frame_rate":"24000/1001","start_time":"0","color_space":"bt709","color_transfer":"bt709","color_primaries":"bt709","color_range":"tv","chroma_location":"left"});
        let document: Document = serde_json::from_value(
            serde_json::json!({"streams":[video(0),video(2)],"format":{"start_time":"0"}}),
        )
        .unwrap();
        let plan = Plan::build(
            &document,
            &document.selected(&[index]).unwrap(),
            &EncodeSettings {
                video_stream_index: index,
                ..Default::default()
            },
        )
        .unwrap();
        (document, plan)
    }
    #[test]
    fn av1an_requires_first_video_instead_of_silently_encoding_another_track() {
        let (document, first) = fixture_plan(0);
        validate_input(&document, &first).unwrap();
        let (_, second) = fixture_plan(2);
        assert_eq!(
            validate_input(&document, &second).unwrap_err().code,
            "ENCODE_INPUT_UNSUPPORTED"
        );
    }
    #[test]
    fn cli_keeps_paths_separate_and_settings_explicit() {
        let (_, plan) = fixture_plan(0);
        let input = Path::new("/media/movie's & $ 日本語.mkv");
        let settings = EncodeSettings {
            film_grain: 8,
            workers: 3,
            ..Default::default()
        };
        let args = arguments(
            input,
            Path::new("/out/owned.ivf"),
            Path::new("/out/work"),
            Path::new("/logs/job.log"),
            &plan,
            &settings,
            false,
            plan.decoder_filter().as_deref(),
        );
        assert!(
            args.windows(2)
                .any(|a| a[0] == "-i" && a[1] == input.as_os_str())
        );
        assert!(args.windows(2).any(|a| a == ["--workers", "3"]));
        assert!(args.windows(2).any(|a| a == ["--chunk-method", "lsmash"]));
        assert!(args.windows(2).any(|a| a == ["--concat", "ffmpeg"]));
        assert!(
            args.windows(2)
                .any(|a| a == ["--audio-params", "-an -sn -dn -map -0:t?"])
        );
        let params = args.windows(2).find(|a| a[0] == "--video-params").unwrap()[1]
            .to_str()
            .unwrap();
        assert!(params.contains("--film-grain 8 --film-grain-denoise 0"));
        assert!(params.contains("--fps-num 24000 --fps-denom 1001"));

        let filtered = arguments(
            input,
            Path::new("/out/owned.ivf"),
            Path::new("/out/work"),
            Path::new("/logs/job.log"),
            &plan,
            &settings,
            false,
            Some("bwdif=mode=send_frame"),
        );
        assert!(
            filtered
                .windows(2)
                .any(|pair| { pair[0] == "--ffmpeg" && pair[1] == "-vf bwdif=mode=send_frame" })
        );
        let targeted_settings = EncodeSettings {
            av1an_options: Some(media_core::Av1anOptions {
                target_quality: Some(media_core::Av1anTargetQuality {
                    metric: media_core::Av1anTargetMetric::Vmaf,
                    minimum_score_tenths: 930,
                    maximum_score_tenths: 950,
                    minimum_crf: 15,
                    maximum_crf: 50,
                    probes: 4,
                    probing_rate: 1,
                    probe_width: 1920,
                    probe_height: 1080,
                }),
                ..Default::default()
            }),
            ..settings.clone()
        };
        let targeted = arguments(
            input,
            Path::new("/out/owned.ivf"),
            Path::new("/out/work"),
            Path::new("/logs/job.log"),
            &plan,
            &targeted_settings,
            false,
            Some("bwdif=mode=send_frame"),
        );
        assert_eq!(
            targeted.iter().filter(|value| *value == "--ffmpeg").count(),
            1
        );
        assert_eq!(
            targeted
                .iter()
                .filter(|value| *value == "--vmaf-filter")
                .count(),
            1
        );
        assert!(
            targeted
                .windows(2)
                .any(|pair| { pair[0] == "--ffmpeg" && pair[1] == "-vf bwdif=mode=send_frame" })
        );
        assert!(
            targeted
                .windows(2)
                .any(|pair| { pair[0] == "--vmaf-filter" && pair[1] == "bwdif=mode=send_frame" })
        );
        let prepared = arguments(
            input,
            Path::new("/out/owned.ivf"),
            Path::new("/out/work"),
            Path::new("/logs/job.log"),
            &plan,
            &settings,
            false,
            None,
        );
        assert!(!prepared.iter().any(|value| value == "--ffmpeg"));
        let targeted_prepared = arguments(
            input,
            Path::new("/out/owned.ivf"),
            Path::new("/out/work"),
            Path::new("/logs/job.log"),
            &plan,
            &targeted_settings,
            false,
            None,
        );
        assert!(
            !targeted_prepared
                .iter()
                .any(|value| value == "--ffmpeg" || value == "--vmaf-filter")
        );
        assert!(!params.contains("movie"));
        assert!(
            !args
                .iter()
                .any(|a| a == "--force" || a == "--ignore-frame-mismatch" || a == "--resume")
        );
    }
    #[test]
    fn chunk_progress_ignores_torn_or_inconsistent_snapshots() {
        assert_eq!(progress(br#"{"frames":236,"done":{"00001":{"frames":55,"size_bytes":42},"00002":{"frames":11}}}"#, 236), Some((66,2)));
        for data in [
            br#"{"frames":236,"done":{"0":{"frames":237}}}"#.as_slice(),
            br#"{"frames":235,"done":{}}"#,
            br#"{"frames":236,"done":{"#,
        ] {
            assert_eq!(progress(data, 236), None);
        }
    }
    #[test]
    fn ivf_header_repair_requires_exact_frame_records_and_leaves_bad_input_untouched() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!(
            "jesses-ivf-{}-{}",
            std::process::id(),
            NEXT_ID.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let path = root.join("owned.ivf");
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let mut bytes = Vec::from(&b"DKIF\0\0\x20\0AV01"[..]);
        bytes.extend(128u16.to_le_bytes());
        bytes.extend(96u16.to_le_bytes());
        bytes.extend(30u32.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(1u32.to_le_bytes());
        bytes.extend(0u32.to_le_bytes());
        for index in 0..3u64 {
            bytes.extend(1u32.to_le_bytes());
            bytes.extend(index.to_le_bytes());
            bytes.push(0);
        }
        file.write_all(&bytes).unwrap();
        let (_tx, cancel) = watch::channel(false);
        assert!(normalize_ivf(&mut file, (128, 96), (24000, 1001), 2, &cancel).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(normalize_ivf(&mut file, (128, 96), (24000, 1001), 3, &cancel).unwrap());
        let repaired = std::fs::read(&path).unwrap();
        assert_eq!(&repaired[16..20], &24000u32.to_le_bytes());
        assert_eq!(&repaired[20..24], &1001u32.to_le_bytes());
        assert_eq!(&repaired[24..28], &3u32.to_le_bytes());
        assert_eq!(&repaired[32..], &bytes[32..]);
        assert!(!normalize_ivf(&mut file, (128, 96), (24000, 1001), 3, &cancel).unwrap());
        drop(file);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
