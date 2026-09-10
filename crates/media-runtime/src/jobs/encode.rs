pub(super) use super::encode_plan::validate_settings;
use super::encode_plan::{Frames, Plan};
use super::*;

impl JobManager {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn encode(
        &self,
        id: &str,
        request: &RemuxRequest,
        settings: &EncodeSettings,
        cancel: &watch::Receiver<bool>,
        log_path: &Path,
        temporary: &mut Option<Temporary>,
        scratch: &mut Vec<Temporary>,
    ) -> Result<(), AppError> {
        check_cancel(cancel)?;
        self.phase(
            id,
            JobState::Preparing,
            "Inspecting selected streams and standalone SVT-AV1 capabilities.",
        )
        .await;
        let input = PathBuf::from(&request.input_path);
        let owned_request = request.clone();
        let (source, output) = tokio::task::spawn_blocking(move || {
            let source = Source::open(&input)?;
            let output = files::output_path(&owned_request, &source)?;
            Ok::<_, AppError>((source, output))
        })
        .await
        .map_err(|e| AppError::new("PREFLIGHT_FAILED", e.to_string(), None))??;
        check_cancel(cancel)?;
        let ffmpeg = discover("ffmpeg", cancel).await?;
        let ffprobe = discover("ffprobe", cancel).await?;
        let encoder = find_executable(&["SvtAv1EncApp", "svtav1encapp"])
            .await
            .map_err(|e| AppError::new("TOOL_DISCOVERY_FAILED", e, None))?
            .ok_or_else(|| {
                AppError::new(
                    "TOOL_MISSING",
                    "The standalone SvtAv1EncApp was not found on PATH.",
                    None,
                )
            })?;
        for (path, version_arg) in [
            (&ffmpeg, "-version"),
            (&ffprobe, "-version"),
            (&encoder, "--version"),
        ] {
            let version = supervisor::run_capture(
                &CommandSpec {
                    executable: path.clone(),
                    args: vec![version_arg.into()],
                    cwd: None,
                },
                cancel.clone(),
                64 * 1024,
                Duration::from_secs(5),
            )
            .await
            .map_err(|e| process_error(e, path))?;
            if !version.status.success() {
                return Err(files::error(
                    "TOOL_FAILED",
                    "A selected media tool failed its version check.",
                    path,
                ));
            }
            let bytes = if version.stdout.is_empty() {
                &version.stderr
            } else {
                &version.stdout
            };
            let version = String::from_utf8_lossy(bytes)
                .lines()
                .find(|line| !line.is_empty())
                .unwrap_or("unknown version")
                .to_owned();
            self.change(id, |snapshot| {
                append_log(snapshot, format!("Tool: {} — {version}", path.display()))
            })
            .await;
        }
        let document = probe(&ffprobe, &source.path, cancel).await?;
        let selected = document.selected(&request.stream_indices)?;
        let plan = Plan::build(&document, &selected, settings)?;
        let video = selected
            .iter()
            .find(|stream| stream.index == plan.video_index)
            .expect("validated video selection");
        self.phase(id,JobState::Preparing,"Decoding the source once to verify every frame timestamp, progressive scan, and SDR format (bounded to 10 minutes and 64 MiB of metadata).").await;
        let source_frames = frame_scan(&ffprobe, &source.path, plan.video_index, cancel).await?;
        let frame_count = plan.validate_frames(&source_frames, video, false)?;
        drop(source_frames);
        source.verify()?;
        check_cancel(cancel)?;
        self.change(id,|snapshot| {
            snapshot.duration_seconds = Some(frame_count as f64*plan.frame_seconds());
            append_log(snapshot,format!("Validated {frame_count} progressive CFR frames at {}/{} fps; CRF {}, preset {}.",plan.fps_num,plan.fps_den,settings.crf,settings.preset));
        }).await;
        tokio::fs::create_dir_all(self.log_dir.as_ref())
            .await
            .map_err(|e| files::error("LOG_CREATE_FAILED", e.to_string(), self.log_dir.as_ref()))?;
        *temporary = Some(Temporary::create(&output, id)?);
        scratch.push(Temporary::create_ivf(&output, &format!("{id}-video"))?);
        let temp = temporary.as_ref().expect("owned Matroska output");
        let ivf = scratch.last().expect("owned IVF output");
        self.change(id, |snapshot| {
            append_log(
                snapshot,
                format!("Owned temporary Matroska: {}", temp.path.display()),
            );
            append_log(
                snapshot,
                format!("Owned temporary AV1: {}", ivf.path.display()),
            );
        })
        .await;
        check_cancel(cancel)?;
        let producer = CommandSpec {
            executable: ffmpeg.clone(),
            args: decoder_args(&source.path, &plan),
            cwd: None,
        };
        let consumer = CommandSpec {
            executable: encoder,
            // SVT's path writer uses exclusive CRT sharing on Windows. Its
            // documented stdout mode lets the supervisor write our owned file
            // handle without releasing the identity guard or narrowing Unicode.
            args: encoder_args(Path::new("stdout"), &plan, settings),
            cwd: None,
        };
        self.phase(id,JobState::Running,"Encoding 10-bit AV1 with the standalone SVT encoder; audio, subtitles, and attachments will be copied afterward.").await;
        let (sender, events) = mpsc::channel(256);
        let event_task = self.observe_encode(id, events, plan.frame_seconds());
        let result = supervisor::run_pipeline_to_file(
            &producer,
            &consumer,
            cancel.clone(),
            sender,
            log_path,
            Duration::from_secs(24 * 60 * 60),
            ivf.clone_file()?,
        )
        .await;
        let _ = event_task.await;
        result.map_err(|e| process_error(e, &source.path))?;
        check_cancel(cancel)?;
        source.verify()?;
        ivf.flush_nonempty_async().await?;
        self.change(id, |snapshot| {
            snapshot.log_path = Some(log_path.to_string_lossy().into_owned())
        })
        .await;
        self.phase(id,JobState::Finalizing,"Muxing encoded video with the selected original audio, subtitles, metadata, chapters, and attachments.").await;
        let mux_log = log_path.with_extension("mux.log");
        let (sender, events) = mpsc::channel(256);
        let observer = self.observe_encode(id, events, plan.frame_seconds());
        let result = supervisor::run(
            &CommandSpec {
                executable: ffmpeg,
                args: mux_args(&source.path, &ivf.path, &temp.path, &selected, &plan),
                cwd: None,
            },
            cancel.clone(),
            sender,
            &mux_log,
            Duration::from_secs(60 * 60),
        )
        .await;
        let _ = observer.await;
        let result = result.map_err(|e| process_error(e, &source.path))?;
        if !result.status.success() {
            return Err(files::error(
                "ENCODE_MUX_FAILED",
                "The encoded video could not be muxed with the selected source tracks.",
                &temp.path,
            ));
        }
        self.change(id, |snapshot| {
            append_log(snapshot, format!("Mux log: {}", mux_log.display()))
        })
        .await;
        check_cancel(cancel)?;
        temp.flush_nonempty_async().await?;
        self.phase(id,JobState::Finalizing,"Decoding the completed AV1 output to verify exact frame count and timing, then checking copied tracks and metadata.").await;
        let artifact = probe(&ffprobe, &temp.path, cancel).await?;
        metadata::verify_encoded(&document, &selected, &artifact, plan.video_index)?;
        let position = selected
            .iter()
            .position(|s| s.index == plan.video_index)
            .expect("selected video");
        let encoded = &artifact.streams[position];
        plan.validate_encoded_stream(video, encoded)?;
        let frames = frame_scan(&ffprobe, &temp.path, encoded.index, cancel).await?;
        let decoded_count = plan
            .validate_frames(&frames, encoded, true)
            .map_err(|error| {
                files::error(
                    "ENCODE_VALIDATION_FAILED",
                    format!("Encoded frame validation failed: {}", error.message),
                    &temp.path,
                )
            })?;
        if decoded_count != frame_count {
            return Err(AppError::new(
                "ENCODE_VALIDATION_FAILED",
                "The encoded video does not decode to exactly the original frame count.",
                None,
            ));
        }
        source.verify()?;
        check_cancel(cancel)?;
        self.finalize(id, cancel, &source, temp, &output).await
    }

    fn observe_encode(
        &self,
        id: &str,
        mut events: mpsc::Receiver<ProcessEvent>,
        frame_seconds: f64,
    ) -> tokio::task::JoinHandle<()> {
        let manager = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                let line = match event {
                    ProcessEvent::Stdout(line) | ProcessEvent::Stderr(line) => line,
                };
                manager
                    .change(&id, |snapshot| {
                        if let Some(value) = svt_frame_counter(&line) {
                            let progress = value as f64 * frame_seconds;
                            snapshot.progress_seconds = Some(
                                snapshot
                                    .duration_seconds
                                    .map_or(progress, |duration| progress.min(duration)),
                            );
                        }
                        append_log(snapshot, line);
                    })
                    .await;
            }
        })
    }
}

fn svt_frame_counter(line: &str) -> Option<u64> {
    line.split_once("Encoding:")
        .map(|(_, rest)| rest)
        .or_else(|| line.split_once("Encoding frame").map(|(_, rest)| rest))?
        .split_whitespace()
        .next()?
        .split('/')
        .next()?
        .parse()
        .ok()
}

async fn frame_scan(
    ffprobe: &Path,
    input: &Path,
    index: u32,
    cancel: &watch::Receiver<bool>,
) -> Result<Frames, AppError> {
    check_cancel(cancel)?;
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-protocol_whitelist",
        "file",
        "-err_detect",
        "explode",
        "-select_streams",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(index.to_string().into());
    args.extend(["-show_frames","-show_entries","frame=best_effort_timestamp_time,interlaced_frame,width,height,pix_fmt,sample_aspect_ratio,chroma_location,color_space,color_transfer,color_primaries,color_range:frame_side_data=side_data_type,rotation","-of","json","-i"].into_iter().map(OsString::from));
    args.push(input.as_os_str().to_owned());
    let output = supervisor::run_capture(
        &CommandSpec {
            executable: ffprobe.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024 * 1024,
        Duration::from_secs(10 * 60),
    )
    .await
    .map_err(|e| process_error(e, input))?;
    check_cancel(cancel)?;
    if !output.status.success() || !output.stderr.is_empty() {
        let detail: String = String::from_utf8_lossy(&output.stderr)
            .chars()
            .take(600)
            .collect();
        return Err(files::error(
            "DECODE_VALIDATION_FAILED",
            format!("The complete video could not be decoded without errors: {detail}"),
            input,
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|_| {
        files::error(
            "PROBE_INVALID_RESPONSE",
            "The frame scan did not return valid metadata.",
            input,
        )
    })
}

fn decoder_args(input: &Path, plan: &Plan) -> Vec<OsString> {
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
    args.push(input.as_os_str().to_owned());
    args.extend(["-map".into(), format!("0:{}", plan.video_index).into()]);
    args.extend(
        [
            "-an",
            "-sn",
            "-dn",
            "-fps_mode",
            "passthrough",
            "-pix_fmt",
            "yuv420p10le",
            "-strict",
            "-1",
            "-f",
            "yuv4mpegpipe",
            "pipe:1",
        ]
        .into_iter()
        .map(OsString::from),
    );
    args
}

fn encoder_args(output: &Path, plan: &Plan, settings: &EncodeSettings) -> Vec<OsString> {
    let values = [
        "--input-depth".into(),
        "10".into(),
        "--crf".into(),
        settings.crf.to_string(),
        "--preset".into(),
        settings.preset.to_string(),
        "--fps-num".into(),
        plan.fps_num.to_string(),
        "--fps-denom".into(),
        plan.fps_den.to_string(),
        "--color-primaries".into(),
        plan.primaries.to_string(),
        "--transfer-characteristics".into(),
        plan.transfer.to_string(),
        "--matrix-coefficients".into(),
        plan.matrix.to_string(),
        "--color-range".into(),
        u8::from(plan.full_range).to_string(),
        "--chroma-sample-position".into(),
        plan.chroma.into(),
    ];
    let mut args: Vec<OsString> = ["-i", "stdin", "--progress", "2", "--passes", "1"]
        .into_iter()
        .map(OsString::from)
        .collect();
    args.extend(values.into_iter().map(OsString::from));
    args.push("-b".into());
    args.push(output.as_os_str().to_owned());
    args
}

fn mux_args(
    input: &Path,
    ivf: &Path,
    output: &Path,
    selected: &[&metadata::Stream],
    plan: &Plan,
) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "warning",
        "-xerror",
        "-protocol_whitelist",
        "file",
        "-i",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    args.push(input.as_os_str().to_owned());
    args.push("-i".into());
    args.push(ivf.as_os_str().to_owned());
    for stream in selected {
        args.extend([
            "-map".into(),
            if stream.index == plan.video_index {
                "1:v:0".into()
            } else {
                format!("0:{}", stream.index).into()
            },
        ]);
    }
    args.extend(
        ["-map_metadata", "0", "-map_chapters", "0", "-c", "copy"]
            .into_iter()
            .map(OsString::from),
    );
    for (index, stream) in selected.iter().enumerate() {
        args.extend([
            format!("-map_metadata:s:{index}").into(),
            format!("0:s:{}", stream.index).into(),
        ]);
        let disposition = stream
            .disposition
            .iter()
            .filter(|(_, v)| **v != 0)
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join("+");
        args.extend([
            format!("-disposition:{index}").into(),
            if disposition.is_empty() {
                "0".into()
            } else {
                disposition.into()
            },
        ]);
        if stream.index == plan.video_index {
            args.extend([
                format!("-aspect:{index}").into(),
                format!("{}:{}", plan.width, plan.height).into(),
            ]);
        }
    }
    args.extend(["-f", "matroska", "-y"].into_iter().map(OsString::from));
    args.push(output.as_os_str().to_owned());
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn parses_supported_standalone_progress_formats() {
        assert_eq!(
            svt_frame_counter("[consumer] Encoding:   12/50 Frames @ 30fps"),
            Some(12)
        );
        assert_eq!(svt_frame_counter("Encoding frame 9 40 fps"), Some(9));
        assert_eq!(svt_frame_counter("Total Encoding Time: 300 ms"), None);
    }

    #[tokio::test]
    #[ignore = "requires real FFmpeg, FFprobe, and standalone SvtAv1EncApp"]
    async fn encodes_fine_timebase_mp4_into_millisecond_matroska() {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = Fixture(
            std::env::temp_dir().join(format!("jesses-encode-mp4-{}-{nonce}", std::process::id())),
        );
        std::fs::create_dir(&fixture.0).unwrap();
        let input = fixture.0.join("source.mp4");
        let output = fixture.0.join("output.mkv");
        let ffmpeg = find_executable(&["ffmpeg"]).await.unwrap().unwrap();
        let mut args:Vec<OsString>=["-v","error","-nostdin","-n","-f","lavfi","-i","testsrc2=s=128x96:r=24","-t","0.5","-vf","setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709","-c:v","libx264","-preset","ultrafast","-bf","0","-pix_fmt","yuv420p","-color_range","tv","-colorspace","bt709","-color_trc","bt709","-color_primaries","bt709","-chroma_sample_location","left"].into_iter().map(OsString::from).collect();
        args.push(input.as_os_str().to_owned());
        let (_sender, cancel) = watch::channel(false);
        let generated = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg,
                args,
                cwd: None,
            },
            cancel,
            64 * 1024,
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let source_bytes = std::fs::read(&input).unwrap();
        let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
        let first = manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: vec![0],
                },
                settings: EncodeSettings {
                    preset: 12,
                    ..EncodeSettings::default()
                },
            })
            .await
            .unwrap();
        let second_output = fixture.0.join("queued-second.mkv");
        let second = manager
            .enqueue_encode(EncodeRequest {
                source: RemuxRequest {
                    output_path: second_output.to_string_lossy().into_owned(),
                    ..first.request.clone()
                },
                settings: first.encode_settings.clone().unwrap(),
            })
            .await
            .unwrap();
        assert_eq!(second.state, JobState::Queued);
        let jobs = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let jobs = manager.list_jobs().await;
                assert!(
                    jobs.iter()
                        .filter(|job| !job.state.is_terminal() && job.state != JobState::Queued)
                        .count()
                        <= 1,
                    "Only one job may execute at a time"
                );
                let first_job = jobs.iter().find(|job| job.id == first.id).unwrap();
                let second_job = jobs.iter().find(|job| job.id == second.id).unwrap();
                if second_job.state != JobState::Queued {
                    assert!(
                        first_job.state.is_terminal(),
                        "Queued work must execute FIFO"
                    );
                }
                if jobs.iter().all(|job| job.state.is_terminal()) {
                    return jobs;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap();
        manager.shutdown().await;
        assert!(
            jobs.iter().all(|job| job.state == JobState::Succeeded),
            "{jobs:#?}"
        );
        drop(manager);
        let restored = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
        let persisted = restored.list_jobs().await;
        assert_eq!(persisted.len(), 2);
        assert!(persisted.iter().all(|job| job.state == JobState::Succeeded
            && job.encode_settings.as_ref() == first.encode_settings.as_ref()));
        restored.shutdown().await;
        assert_eq!(std::fs::read(&input).unwrap(), source_bytes);
        assert!(output.exists());

        let canceled_output = fixture.0.join("canceled.mkv");
        let cancel_manager = JobManager::new(fixture.0.join("cancel-logs"));
        let cancel_job = cancel_manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: canceled_output.to_string_lossy().into_owned(),
                    stream_indices: vec![0],
                },
                settings: EncodeSettings {
                    preset: 0,
                    ..EncodeSettings::default()
                },
            })
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let job = cancel_manager.list_jobs().await.remove(0);
                assert!(
                    !job.state.is_terminal(),
                    "Expected to observe the live encoder before completion: {job:#?}"
                );
                if job.state == JobState::Running
                    && job
                        .logs
                        .iter()
                        .any(|line| line.contains("[consumer] Svt[info]"))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        cancel_manager.cancel_job(cancel_job.id).await.unwrap();
        cancel_manager.shutdown().await;
        let canceled = cancel_manager.list_jobs().await.remove(0);
        assert_eq!(canceled.state, JobState::Canceled, "{canceled:#?}");
        assert!(!canceled_output.exists());
        assert!(!std::fs::read_dir(&fixture.0).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".partial.")
        }));
        assert_eq!(std::fs::read(input).unwrap(), source_bytes);
    }

    #[tokio::test]
    #[ignore = "requires real FFmpeg, FFprobe, and standalone SvtAv1EncApp"]
    async fn standalone_encode_preserves_frame_timing_copied_tracks_and_source() {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let fixture = Fixture(
            std::env::temp_dir().join(format!("jesses-encode-{}-{nonce}", std::process::id())),
        );
        std::fs::create_dir(&fixture.0).unwrap();
        let input = fixture.0.join("source's & 日本語.mkv");
        let output = fixture.0.join("output's & 日本語.mkv");
        let chapters = fixture.0.join("chapters.txt");
        let subtitles = fixture.0.join("subtitles.srt");
        let attachment = fixture.0.join("note.txt");
        std::fs::write(&chapters,";FFMETADATA1\ntitle=Encode fixture\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=500\ntitle=Opening\n").unwrap();
        std::fs::write(&subtitles, "1\n00:00:00,000 --> 00:00:00,450\nA subtitle\n").unwrap();
        std::fs::write(&attachment, "Copied attachment bytes").unwrap();
        let ffmpeg = find_executable(&["ffmpeg"]).await.unwrap().unwrap();
        let mut args: Vec<OsString> = [
            "-v",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=128x96:r=24000/1001",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000",
            "-i",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        args.push(subtitles.into_os_string());
        args.extend(["-f", "ffmetadata", "-i"].into_iter().map(OsString::from));
        args.push(chapters.into_os_string());
        args.extend(
            [
                "-t",
                "0.5",
                "-map",
                "0:v",
                "-map",
                "1:a",
                "-map",
                "2:s",
                "-map_metadata",
                "3",
                "-map_chapters",
                "3",
                "-c:v",
                "ffv1",
                "-vf",
                "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
                "-pix_fmt",
                "yuv420p",
                "-color_range",
                "tv",
                "-colorspace",
                "bt709",
                "-color_trc",
                "bt709",
                "-color_primaries",
                "bt709",
                "-chroma_sample_location",
                "left",
                "-c:a",
                "pcm_s16le",
                "-c:s",
                "srt",
                "-metadata:s:v:0",
                "title=Picture",
                "-metadata:s:a:0",
                "language=jpn",
                "-metadata:s:a:0",
                "title=Original audio",
                "-metadata:s:s:0",
                "language=eng",
                "-disposition:s:0",
                "forced",
                "-attach",
            ]
            .into_iter()
            .map(OsString::from),
        );
        args.push(attachment.into_os_string());
        args.extend(
            ["-metadata:s:t:0", "mimetype=text/plain"]
                .into_iter()
                .map(OsString::from),
        );
        args.push(input.as_os_str().to_owned());
        let (_sender, cancel) = watch::channel(false);
        let generated = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg,
                args,
                cwd: None,
            },
            cancel,
            64 * 1024,
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        assert!(
            generated.status.success(),
            "{}",
            String::from_utf8_lossy(&generated.stderr)
        );
        let source_bytes = std::fs::read(&input).unwrap();
        let manager = JobManager::new(fixture.0.join("logs"));
        manager
            .start_encode(EncodeRequest {
                source: RemuxRequest {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    stream_indices: vec![1, 0, 2, 3],
                },
                settings: EncodeSettings {
                    preset: 12,
                    ..EncodeSettings::default()
                },
            })
            .await
            .unwrap();
        let job = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                let job = manager.list_jobs().await.remove(0);
                if job.state.is_terminal() {
                    return job;
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        })
        .await
        .unwrap();
        manager.shutdown().await;
        assert_eq!(job.state, JobState::Succeeded, "{job:#?}");
        assert_eq!(std::fs::read(input).unwrap(), source_bytes);
        let media = crate::probe_media(output.to_string_lossy().into_owned())
            .await
            .unwrap();
        assert_eq!(
            media
                .streams
                .iter()
                .map(|s| s.kind.as_str())
                .collect::<Vec<_>>(),
            ["audio", "video", "subtitle", "attachment"]
        );
        assert_eq!(media.streams[1].codec.as_deref(), Some("av1"));
        assert!(!std::fs::read_dir(&fixture.0).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".partial.")
        }));
    }
}
