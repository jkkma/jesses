use super::*;
use std::fs;

static NEXT_FIXTURE_ID: AtomicU64 = AtomicU64::new(0);

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        for _ in 0..100 {
            let serial = NEXT_FIXTURE_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "jesses-atomic-batch-{}-{nonce}-{serial}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => {
                    fs::write(path.join("source.mkv"), b"unchanged source").unwrap();
                    return Self(path);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!(
                    "Cannot create batch test fixture {}: {error}",
                    path.display()
                ),
            }
        }
        panic!("Could not reserve a unique batch test fixture directory");
    }
    fn request(&self, index: usize) -> EncodeRequest {
        EncodeRequest {
            source: RemuxRequest {
                input_path: self.0.join("source.mkv").to_string_lossy().into_owned(),
                output_path: self
                    .0
                    .join(format!("output-{index}.mkv"))
                    .to_string_lossy()
                    .into_owned(),
                stream_indices: vec![0],
            },
            settings: EncodeSettings {
                crf: 30 + (index % 3) as u8,
                preset: 4,
                ..EncodeSettings::default()
            },
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

// Transaction-boundary tests bypass probing so they run without external tools.
// The execution slot stays held until all admitted jobs have been canceled.
#[tokio::test]
async fn batch_refuses_collisions_without_queuing_or_altering_history() {
    let fixture = Fixture::new();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    let slot = manager.execution.lock().await;
    let before = fs::read(fixture.0.join("history/jobs.json")).unwrap();
    fs::write(fixture.0.join("output-1.mkv"), b"existing output").unwrap();
    let error = manager
        .admit_encode_batch(vec![fixture.request(0), fixture.request(1)])
        .await
        .unwrap_err();
    assert_eq!(error.code, "OUTPUT_EXISTS");
    assert!(manager.list_jobs().await.is_empty());
    assert_eq!(
        fs::read(fixture.0.join("history/jobs.json")).unwrap(),
        before
    );
    let first = fixture.request(2);
    let mut alias = fixture.request(3);
    alias.source.output_path = fixture
        .0
        .join(".")
        .join("output-2.mkv")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        manager
            .admit_encode_batch(vec![first, alias])
            .await
            .unwrap_err()
            .code,
        "OUTPUT_QUEUED"
    );
    assert!(manager.list_jobs().await.is_empty());
    assert_eq!(
        fs::read(fixture.0.join("output-1.mkv")).unwrap(),
        b"existing output"
    );
    assert_eq!(
        fs::read(fixture.0.join("source.mkv")).unwrap(),
        b"unchanged source"
    );
    drop(slot);
    manager.shutdown().await;
}

#[tokio::test]
async fn batch_capacity_failure_keeps_the_entire_existing_queue() {
    let fixture = Fixture::new();
    let manager = JobManager::new(fixture.0.join("logs"));
    let slot = manager.execution.lock().await;
    let existing = manager
        .admit_encode_batch((0..99).map(|i| fixture.request(i)).collect())
        .await
        .unwrap();
    assert_eq!(
        manager
            .admit_encode_batch(vec![fixture.request(99), fixture.request(100)])
            .await
            .unwrap_err()
            .code,
        "QUEUE_FULL"
    );
    let after = manager.list_jobs().await;
    assert_eq!(after.len(), 99);
    assert_eq!(
        after.iter().rev().map(|job| &job.id).collect::<Vec<_>>(),
        existing.iter().map(|job| &job.id).collect::<Vec<_>>()
    );
    manager.cancel_all_jobs().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), manager.shutdown())
        .await
        .unwrap();
    assert!(
        manager
            .list_jobs()
            .await
            .iter()
            .all(|job| job.state == JobState::Canceled)
    );
    drop(slot);
}

#[tokio::test]
async fn batch_persists_all_items_before_spawn_and_stop_sees_all_or_none() {
    let fixture = Fixture::new();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    let slot = manager.execution.lock().await;
    let mut requests = vec![fixture.request(0), fixture.request(1)];
    requests[0].settings.framing.crop.left = 16;
    requests[1].settings.framing.resize_width = Some(960);
    let (admitted, stopped) = tokio::join!(
        manager.admit_encode_batch(requests.clone()),
        manager.cancel_all_jobs()
    );
    let admitted = admitted.unwrap();
    let stopped = stopped.unwrap();
    assert!(
        stopped.is_empty() || stopped.len() == 2,
        "Stop cannot observe a half-admitted batch"
    );
    assert_eq!(
        admitted
            .iter()
            .map(|job| job.encode_settings.clone().unwrap())
            .collect::<Vec<_>>(),
        requests
            .iter()
            .map(|request| request.settings.clone())
            .collect::<Vec<_>>()
    );
    let record: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.0.join("history/jobs.json")).unwrap()).unwrap();
    assert_eq!(record["jobs"].as_array().unwrap().len(), 2);
    assert_eq!(record["jobs"][0]["id"], admitted[0].id);
    assert_eq!(record["jobs"][1]["id"], admitted[1].id);
    assert_eq!(
        record["jobs"][0]["encodeSettings"]["framing"]["crop"]["left"],
        16
    );
    assert_eq!(
        record["jobs"][1]["encodeSettings"]["framing"]["resizeWidth"],
        960
    );
    assert!(manager.list_jobs().await.iter().all(|job| matches!(
        job.state,
        JobState::Queued | JobState::Canceling | JobState::Canceled
    )));
    manager.cancel_all_jobs().await.unwrap();
    manager.shutdown().await;
    assert!(
        manager
            .list_jobs()
            .await
            .iter()
            .all(|job| job.state == JobState::Canceled)
    );
    drop(slot);
}

#[tokio::test]
async fn failed_batch_history_write_never_admits_or_spawns_items() {
    let fixture = Fixture::new();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    let history = fixture.0.join("history/jobs.json");
    let saved = fixture.0.join("history/saved.json");
    let before = fs::read(&history).unwrap();
    fs::rename(&history, &saved).unwrap();
    fs::create_dir(&history).unwrap();
    assert_eq!(
        manager
            .admit_encode_batch(vec![fixture.request(0), fixture.request(1)])
            .await
            .unwrap_err()
            .code,
        "JOB_HISTORY_FAILED"
    );
    assert!(manager.list_jobs().await.is_empty());
    assert_eq!(fs::read(saved).unwrap(), before);
    assert!(!fixture.0.join("output-0.mkv").exists());
    assert!(!fixture.0.join("output-1.mkv").exists());
    manager.shutdown().await;
}

#[tokio::test]
async fn batch_reports_each_failure_and_continues_to_the_next_item() {
    let fixture = Fixture::new();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    let slot = manager.execution.lock().await;
    let first_source = fixture.0.join("first-source.mkv");
    let second_source = fixture.0.join("second-source.mkv");
    fs::write(&first_source, b"first source").unwrap();
    fs::write(&second_source, b"second source").unwrap();
    let mut first = fixture.request(0);
    first.source.input_path = first_source.to_string_lossy().into_owned();
    let mut second = fixture.request(1);
    second.source.input_path = second_source.to_string_lossy().into_owned();
    let admitted = manager
        .admit_encode_batch(vec![first, second])
        .await
        .unwrap();

    // Both requests are already durably admitted. Give each a distinct,
    // deterministic execution failure that occurs before tool discovery.
    fs::remove_file(&first_source).unwrap();
    fs::write(fixture.0.join("output-1.mkv"), b"existing destination").unwrap();
    drop(slot);

    let jobs = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let jobs = manager.list_jobs().await;
            if jobs.iter().all(|job| job.state.is_terminal()) {
                break jobs;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("both failed batch items must reach a reported terminal state");
    let first = jobs
        .iter()
        .find(|job| job.id == admitted[0].id)
        .expect("first admitted result");
    let second = jobs
        .iter()
        .find(|job| job.id == admitted[1].id)
        .expect("second admitted result");
    assert_eq!(first.state, JobState::Failed);
    assert_eq!(first.error.as_ref().unwrap().code, "FILE_UNREADABLE");
    assert_eq!(second.state, JobState::Failed);
    assert_eq!(second.error.as_ref().unwrap().code, "OUTPUT_EXISTS");
    for job in [first, second] {
        assert!(
            job.logs
                .iter()
                .any(|line| line.contains("Inspecting selected streams")),
            "every admitted item must execute and report its own result: {job:#?}"
        );
    }
    let persisted: serde_json::Value =
        serde_json::from_slice(&fs::read(fixture.0.join("history/jobs.json")).unwrap()).unwrap();
    let persisted = persisted["jobs"].as_array().unwrap();
    assert!(admitted.iter().all(|job| persisted.iter().any(|saved| {
        saved["id"] == job.id && saved["state"] == "failed" && saved["error"].is_object()
    })));
    assert!(!fixture.0.join("output-0.mkv").exists());
    assert_eq!(
        fs::read(fixture.0.join("output-1.mkv")).unwrap(),
        b"existing destination"
    );
    assert_eq!(fs::read(second_source).unwrap(), b"second source");
    manager.shutdown().await;
}

#[tokio::test]
async fn stop_queue_cancels_every_batch_item_waiting_between_files() {
    let fixture = Fixture::new();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    let slot = manager.execution.lock().await;
    let admitted = manager
        .admit_encode_batch((0..3).map(|index| fixture.request(index)).collect())
        .await
        .unwrap();
    let stopped = manager.cancel_all_jobs().await.unwrap();
    assert_eq!(stopped.len(), admitted.len());
    tokio::time::timeout(Duration::from_secs(2), manager.shutdown())
        .await
        .expect("waiting batch workers must stop without acquiring the execution slot");
    let jobs = manager.list_jobs().await;
    assert_eq!(jobs.len(), admitted.len());
    for admitted in admitted {
        let result = jobs
            .iter()
            .find(|job| job.id == admitted.id)
            .expect("one result per stopped batch item");
        assert_eq!(result.state, JobState::Canceled);
        assert!(
            result
                .logs
                .iter()
                .any(|line| line.contains("Queued job stopped before starting"))
        );
        assert!(
            !result
                .logs
                .iter()
                .any(|line| line.contains("Inspecting selected streams"))
        );
        assert!(!Path::new(&result.request.output_path).exists());
    }
    drop(slot);
}

#[tokio::test]
async fn stop_invalidates_a_batch_preflight_but_allows_later_submissions() {
    let fixture = Fixture::new();
    let manager = JobManager::new(fixture.0.join("logs"));
    let slot = manager.execution.lock().await;
    let preflight_epoch = manager.state.lock().await.submission_epoch;
    manager.cancel_all_jobs().await.unwrap();
    let error = manager
        .admit_encode_batch_at_epoch(
            vec![fixture.request(0), fixture.request(1)],
            preflight_epoch,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "BATCH_CANCELED");
    assert!(manager.list_jobs().await.is_empty());
    let admitted = manager
        .admit_encode_batch(vec![fixture.request(2)])
        .await
        .unwrap();
    assert_eq!(admitted.len(), 1);
    manager.cancel_all_jobs().await.unwrap();
    manager.shutdown().await;
    drop(slot);
}

#[tokio::test]
async fn single_submission_respects_canonical_batch_destination_reservations() {
    let fixture = Fixture::new();
    let manager = JobManager::new(fixture.0.join("logs"));
    let slot = manager.execution.lock().await;
    let request = fixture.request(0);
    manager
        .admit_encode_batch(vec![request.clone()])
        .await
        .unwrap();
    assert_eq!(
        manager
            .enqueue_encode(request.clone())
            .await
            .unwrap_err()
            .code,
        "OUTPUT_QUEUED"
    );
    let mut alias = request;
    alias.source.output_path = fixture
        .0
        .join(".")
        .join("output-0.mkv")
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        manager.enqueue_encode(alias).await.unwrap_err().code,
        "OUTPUT_QUEUED"
    );
    assert_eq!(manager.list_jobs().await.len(), 1);
    manager.cancel_all_jobs().await.unwrap();
    manager.shutdown().await;
    drop(slot);
}

#[tokio::test]
async fn batch_preflight_processes_stop_and_shutdown_awaits_dropped_requests() {
    for closing in [false, true] {
        let fixture = Fixture::new();
        fs::write(fixture.0.join("mode"), "sleep").unwrap();
        let manager = JobManager::new(fixture.0.join("logs"));
        let worker = manager.clone();
        let path = fixture.0.clone();
        let request = tokio::spawn(async move {
            worker
                .run_preflight(0, move |cancel| async move {
                    let result = supervisor::run_capture(
                        &CommandSpec {
                            executable: std::env::current_exe().unwrap(),
                            args: [
                                "--exact",
                                "supervisor::tests::fake_tool",
                                "--ignored",
                                "--nocapture",
                            ]
                            .into_iter()
                            .map(OsString::from)
                            .collect(),
                            cwd: Some(path.clone()),
                        },
                        cancel,
                        64 * 1024,
                        Duration::from_secs(30),
                    )
                    .await
                    .map(|_| ())
                    .map_err(|e| process_error(e, &path));
                    fs::write(path.join("cleanup-finished"), b"done").unwrap();
                    result
                })
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !fixture.0.join("pid").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(
            manager.list_jobs().await.is_empty(),
            "preflight has not admitted a job"
        );
        if closing {
            // The native caller can disappear while its source probe is alive.
            // The manager still owns and awaits that process before app exit.
            request.abort();
            tokio::time::timeout(Duration::from_secs(5), manager.shutdown())
                .await
                .unwrap();
            assert!(fixture.0.join("cleanup-finished").exists());
            assert_eq!(
                manager
                    .run_preflight(0, |_| async { Ok(()) })
                    .await
                    .unwrap_err()
                    .code,
                "APP_CLOSING"
            );
        } else {
            manager.cancel_all_jobs().await.unwrap();
            assert_eq!(
                tokio::time::timeout(Duration::from_secs(5), request)
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap_err()
                    .code,
                "BATCH_CANCELED"
            );
            assert!(fixture.0.join("cleanup-finished").exists());
            manager.shutdown().await;
        }
    }
}

#[test]
fn batch_header_preflight_rejects_unsupported_encoding_before_queueing() {
    let input = BatchEncodeInput {
        temporal: None,
        tone_map: None,
        trim: None,
        subtitles: Vec::new(),
        framing: Default::default(),
        audio: Vec::new(),
        input_path: "source.mkv".into(),
        stream_indices: vec![2],
        video_stream_index: 2,
    };
    let document = serde_json::json!({
        "streams": [{
            "index":2,"codec_type":"video","codec_name":"h264",
            "width":128,"height":96,"pix_fmt":"yuv420p","field_order":"progressive",
            "sample_aspect_ratio":"1:1","avg_frame_rate":"24000/1001",
            "time_base":"1/1000","start_time":"0","color_range":"tv",
            "color_space":"bt709","color_transfer":"bt709","color_primaries":"bt709",
            "chroma_location":"left"
        }],
        "format":{"start_time":"0"}
    });
    validate_encode_preview(
        &serde_json::to_vec(&document).unwrap(),
        &input,
        &EncodeSettings::default(),
    )
    .unwrap();
    for (field, value) in [
        ("color_transfer", serde_json::json!("smpte2084")),
        ("field_order", serde_json::json!("tt")),
        ("width", serde_json::json!(127)),
        ("sample_aspect_ratio", serde_json::json!("4:3")),
    ] {
        let mut unsupported = document.clone();
        unsupported["streams"][0][field] = value;
        assert_eq!(
            validate_encode_preview(
                &serde_json::to_vec(&unsupported).unwrap(),
                &input,
                &EncodeSettings::default()
            )
            .unwrap_err()
            .code,
            "ENCODE_INPUT_UNSUPPORTED",
            "{field}"
        );
    }
}

#[test]
fn batch_header_preflight_rejects_alternate_video_only_for_av1an() {
    let video = |index| {
        serde_json::json!({
            "index":index,"codec_type":"video","codec_name":"h264",
            "width":128,"height":96,"pix_fmt":"yuv420p","field_order":"progressive",
            "sample_aspect_ratio":"1:1","avg_frame_rate":"24000/1001",
            "time_base":"1/1000","start_time":"0","color_range":"tv",
            "color_space":"bt709","color_transfer":"bt709","color_primaries":"bt709",
            "chroma_location":"left"
        })
    };
    let bytes = serde_json::to_vec(&serde_json::json!({
        "streams":[{"index":0,"codec_type":"audio","codec_name":"aac"},video(2),video(9)],
        "format":{"start_time":"0"}
    }))
    .unwrap();
    let av1an = EncodeSettings {
        backend: media_core::EncodeBackend::Av1an,
        ..EncodeSettings::default()
    };
    for index in [2, 9] {
        let input = BatchEncodeInput {
            temporal: None,
            tone_map: None,
            trim: None,
            subtitles: Vec::new(),
            framing: Default::default(),
            audio: Vec::new(),
            input_path: "multi-video.mkv".into(),
            stream_indices: vec![index, 0],
            video_stream_index: index,
        };
        validate_encode_preview(&bytes, &input, &EncodeSettings::default()).unwrap();
        let result = validate_encode_preview(&bytes, &input, &av1an);
        if index == 2 {
            result.unwrap();
        } else {
            let error = result.unwrap_err();
            assert_eq!(error.code, "ENCODE_INPUT_UNSUPPORTED");
            assert!(error.message.contains("first video track"));
        }
    }
}

#[test]
fn batch_header_preflight_honors_explicit_hdr10_fallback_for_both_backends() {
    let input = BatchEncodeInput {
        temporal: None,
        tone_map: None,
        trim: None,
        subtitles: Vec::new(),
        framing: Default::default(),
        audio: Vec::new(),
        input_path: "hdr-base.mkv".into(),
        stream_indices: vec![2],
        video_stream_index: 2,
    };
    // Mastering metadata may be carried by the first decoded frame. Header
    // preview must preserve consent and defer that full scan to execution.
    let document = serde_json::json!({
        "streams":[{
            "index":2,"codec_type":"video","codec_name":"hevc",
            "width":3840,"height":2160,"pix_fmt":"yuv420p10le","field_order":"progressive",
            "sample_aspect_ratio":"1:1","avg_frame_rate":"24000/1001",
            "time_base":"1/1000","start_time":"0","color_range":"tv",
            "color_space":"bt2020nc","color_transfer":"smpte2084","color_primaries":"bt2020",
            "chroma_location":"topleft",
            "side_data_list":[
                {"side_data_type":"DOVI configuration record","dv_profile":7,
                 "dv_bl_signal_compatibility_id":6,"bl_present_flag":1,
                 "rpu_present_flag":1,"el_present_flag":1},
                {"side_data_type":"HEVC enhancement-layer decoder configuration"}
            ]
        }],
        "format":{"start_time":"0"}
    });
    let bytes = serde_json::to_vec(&document).unwrap();
    for backend in [
        media_core::EncodeBackend::Standalone,
        media_core::EncodeBackend::Av1an,
    ] {
        let settings = EncodeSettings {
            backend,
            ..EncodeSettings::default()
        };
        let error = validate_encode_preview(&bytes, &input, &settings).unwrap_err();
        assert_eq!(error.code, "ENCODE_INPUT_UNSUPPORTED");
        assert!(error.message.contains("explicit HDR10 fallback"));
        let allowed = EncodeSettings {
            hdr10_fallback: true,
            ..settings
        };
        validate_encode_preview(&bytes, &input, &allowed).unwrap();

        let mut incompatible = document.clone();
        incompatible["streams"][0]["side_data_list"][0]["dv_profile"] = 5.into();
        assert_eq!(
            validate_encode_preview(
                &serde_json::to_vec(&incompatible).unwrap(),
                &input,
                &allowed
            )
            .unwrap_err()
            .code,
            "ENCODE_INPUT_UNSUPPORTED"
        );
    }
}

#[tokio::test]
#[ignore = "requires FFmpeg, FFprobe, and standalone SvtAv1EncApp"]
async fn preview_and_atomic_batch_preserve_selections_and_execute_fifo() {
    let fixture = Fixture::new();
    let input = fixture.0.join("A movie 日本語.mp4");
    let ffmpeg = crate::discovery::find_executable(&["ffmpeg"])
        .await
        .unwrap()
        .unwrap();
    let mut args: Vec<OsString> = [
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=128x96:r=24",
        "-t",
        "0.5",
        "-vf",
        "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-bf",
        "0",
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
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
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
    let before = fs::read(&input).unwrap();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    let slot = manager.execution.lock().await;
    let selected = BatchEncodeInput {
        temporal: None,
        tone_map: None,
        trim: None,
        subtitles: Vec::new(),
        framing: Default::default(),
        audio: Vec::new(),
        input_path: input.to_string_lossy().into_owned(),
        stream_indices: vec![0],
        video_stream_index: 0,
    };
    let preview = manager
        .preview_encode_batch(BatchEncodeRequest {
            parameters: Vec::new(),
            av1an_options: None,
            av1an_grain: None,
            av1an_filters: Vec::new(),
            output_container: None,
            rate_control: None,
            lossless: false,
            svt_crf_quarter_steps: None,
            svt_preset: None,
            encoder: media_core::VideoEncoder::SvtAv1,
            inputs: vec![
                selected.clone(),
                selected.clone(),
                BatchEncodeInput {
                    framing: Default::default(),
                    audio: Vec::new(),
                    stream_indices: vec![99],
                    ..selected
                },
            ],
            output_directory: fixture.0.to_string_lossy().into_owned(),
            output_name_template: None,
            naming_date: None,
            crf: 30,
            preset: 12,
            film_grain: 0,
            lineart_psy_bias: 0,
            texture_psy_bias: 0,
            hdr_tune: Default::default(),
            hdr10_fallback: false,
            backend: Default::default(),
            workers: 2,
        })
        .await
        .unwrap();
    assert_eq!(
        preview.items[2].error.as_ref().unwrap().code,
        "STREAM_NOT_FOUND"
    );
    assert!(
        preview.items[0]
            .output_path
            .as_ref()
            .unwrap()
            .ends_with("A movie 日本語_av1.mkv")
    );
    assert!(
        preview.items[1]
            .output_path
            .as_ref()
            .unwrap()
            .ends_with("A movie 日本語_av1_2.mkv")
    );
    let requests: Vec<_> = preview
        .items
        .into_iter()
        .filter_map(|item| item.request)
        .collect();
    let mut invalid = requests[1].clone();
    invalid.settings.video_stream_index = 99;
    assert_eq!(
        manager
            .enqueue_encode_batch(vec![requests[0].clone(), invalid])
            .await
            .unwrap_err()
            .code,
        "STREAM_SELECTION_INVALID"
    );
    assert!(manager.list_jobs().await.is_empty());
    assert!(!Path::new(&requests[0].source.output_path).exists());
    let admitted = manager
        .enqueue_encode_batch(requests.clone())
        .await
        .unwrap();
    let queued_preview = manager
        .preview_encode_batch(BatchEncodeRequest {
            parameters: Vec::new(),
            av1an_options: None,
            av1an_grain: None,
            av1an_filters: Vec::new(),
            output_container: None,
            rate_control: None,
            lossless: false,
            svt_crf_quarter_steps: None,
            svt_preset: None,
            encoder: media_core::VideoEncoder::SvtAv1,
            inputs: vec![BatchEncodeInput {
                temporal: None,
                tone_map: None,
                trim: None,
                subtitles: Vec::new(),
                framing: Default::default(),
                audio: Vec::new(),
                input_path: input.to_string_lossy().into_owned(),
                stream_indices: vec![0],
                video_stream_index: 0,
            }],
            output_directory: fixture.0.to_string_lossy().into_owned(),
            output_name_template: None,
            naming_date: None,
            crf: 30,
            preset: 12,
            film_grain: 0,
            lineart_psy_bias: 0,
            texture_psy_bias: 0,
            hdr_tune: Default::default(),
            hdr10_fallback: false,
            backend: Default::default(),
            workers: 2,
        })
        .await
        .unwrap();
    assert!(
        queued_preview.items[0]
            .output_path
            .as_ref()
            .unwrap()
            .ends_with("A movie 日本語_av1_3.mkv")
    );
    assert_eq!(
        manager
            .enqueue_encode_batch(requests.clone())
            .await
            .unwrap_err()
            .code,
        "OUTPUT_QUEUED"
    );
    assert_eq!(manager.list_jobs().await.len(), 2);
    drop(slot);
    let jobs = tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            let jobs = manager.list_jobs().await;
            assert!(
                jobs.iter()
                    .filter(|job| !job.state.is_terminal() && job.state != JobState::Queued)
                    .count()
                    <= 1
            );
            let first = jobs.iter().find(|job| job.id == admitted[0].id).unwrap();
            let second = jobs.iter().find(|job| job.id == admitted[1].id).unwrap();
            if second.state != JobState::Queued {
                assert!(first.state.is_terminal());
            }
            if jobs.iter().all(|job| job.state.is_terminal()) {
                return jobs;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    manager.shutdown().await;
    assert!(
        jobs.iter().all(|job| job.state == JobState::Succeeded),
        "{jobs:#?}"
    );
    assert_eq!(fs::read(input).unwrap(), before);
}
