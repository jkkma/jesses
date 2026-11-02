//! CPU-only admission must be independent of installed hardware/tool discovery.
use media_core::*;
use media_runtime::{JobManager, get_encoder_parameters, preview_encode_plan, run_utility};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-artifacts/cpu-only")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn request(&self, encoder: VideoEncoder) -> EncodeRequest {
        EncodeRequest {
            source: RemuxRequest {
                input_path: self.0.join("source.mkv").to_string_lossy().into_owned(),
                output_path: self.0.join("out.mkv").to_string_lossy().into_owned(),
                stream_indices: vec![0],
            },
            settings: EncodeSettings {
                encoder,
                ..Default::default()
            },
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("Retained {}", self.0.display());
        } else {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
fn cancel() -> tokio::sync::watch::Receiver<bool> {
    static OWNER: std::sync::OnceLock<tokio::sync::watch::Sender<bool>> =
        std::sync::OnceLock::new();
    OWNER
        .get_or_init(|| tokio::sync::watch::channel(false).0)
        .subscribe()
}
fn unsupported(error: AppError) {
    assert_eq!(error.code, "HARDWARE_ENCODING_UNSUPPORTED", "{error:?}");
}
fn batch(request: &EncodeRequest) -> BatchEncodeRequest {
    serde_json::from_value(serde_json::json!({"encoder":request.settings.encoder,"inputs":[{"inputPath":request.source.input_path,"streamIndices":[0],"videoStreamIndex":0}],"outputDirectory":PathBuf::from(&request.source.output_path).parent().unwrap(),"crf":30,"preset":4})).unwrap()
}
#[tokio::test]
async fn normal_held_batch_and_parameter_requests_reject_hardware_without_tools_or_queue_mutation()
{
    let fixture = Fixture::new();
    let manager = JobManager::new(fixture.0.join("logs"));
    for encoder in [VideoEncoder::H264Nvenc, VideoEncoder::HevcNvenc] {
        let request = fixture.request(encoder);
        unsupported(manager.start_encode(request.clone()).await.unwrap_err());
        unsupported(manager.enqueue_encode(request.clone()).await.unwrap_err());
        unsupported(
            manager
                .prepare_command_edit(
                    CommandEditRequest::Encode {
                        request: Box::new(request.clone()),
                    },
                    cancel(),
                )
                .await
                .unwrap_err(),
        );
        unsupported(
            preview_encode_plan(request.clone(), cancel())
                .await
                .unwrap_err(),
        );
        unsupported(
            get_encoder_parameters(encoder, EncodeBackend::Standalone, cancel())
                .await
                .unwrap_err(),
        );
        unsupported(
            get_encoder_parameters(encoder, EncodeBackend::Av1an, cancel())
                .await
                .unwrap_err(),
        );
        unsupported(
            manager
                .preview_encode_batch(batch(&request))
                .await
                .unwrap_err(),
        );
        unsupported(
            manager
                .enqueue_encode_batch(vec![request])
                .await
                .unwrap_err(),
        );
        assert!(manager.list_jobs().await.is_empty());
    }
    // A historical source-profile marker cannot activate a hidden hardware route
    // under a replacement CPU encoder either.
    let mut request = fixture.request(VideoEncoder::X264);
    request.settings.source_nvenc_quality = true;
    unsupported(manager.start_encode(request).await.unwrap_err());
    manager.cancel_command_reviews().await.unwrap();
    manager.shutdown().await;
}
#[tokio::test]
async fn sample_encodes_and_utility_batch_reject_legacy_hardware_requests() {
    for encoder in [LadderEncoder::H264Nvenc, LadderEncoder::HevcNvenc] {
        let request = UtilityRequest::CrfLadder(CrfLadderRequest {
            input_path: "/missing/source.mkv".into(),
            video_stream_index: 0,
            encoder,
            preset: "p4".into(),
            pixel_format: "yuv420p".into(),
            crfs: vec![20],
            sample_count: 1,
            sample_seconds: 1.,
            metric: LadderMetric::None,
            recommendation_threshold: None,
            content_preset: None,
            parameters: vec![],
            vmaf_model: None,
            keep_samples_directory: None,
        });
        unsupported(run_utility(request.clone(), cancel()).await.unwrap_err());
        let result = media_runtime::run_utility_batch(
            UtilityBatchRequest {
                requests: vec![request],
            },
            cancel(),
        )
        .await
        .unwrap();
        unsupported(result.entries[0].error.clone().unwrap());
        assert_eq!(result.completed, 0);
    }
}
#[tokio::test]
async fn old_hardware_history_stays_readable_and_saved_new_and_held_resume_fail_without_changes() {
    let fixture = Fixture::new();
    let history = fixture.0.join("history");
    fs::create_dir(&history).unwrap();
    let mut settings = fixture.request(VideoEncoder::H264Nvenc).settings;
    settings.backend = EncodeBackend::Av1an;
    let job:JobSnapshot=serde_json::from_value(serde_json::json!({"id":"old-nvenc","state":"stopped","request":fixture.request(VideoEncoder::H264Nvenc).source,"encodeSettings":settings,"progressSeconds":null,"durationSeconds":null,"logs":["Historical hardware job"],"error":null,"logPath":null})).unwrap();
    fs::write(
        history.join("jobs.json"),
        serde_json::to_vec(&serde_json::json!({"version":1,"jobs":[job]})).unwrap(),
    )
    .unwrap();
    let manager = JobManager::open(fixture.0.join("logs"), history.clone()).await;
    manager.ready().await.unwrap();
    let before = manager.list_jobs().await;
    assert_eq!(
        before[0].encode_settings.as_ref().unwrap().encoder,
        VideoEncoder::H264Nvenc
    );
    unsupported(manager.resume_job("old-nvenc".into()).await.unwrap_err());
    unsupported(
        manager
            .prepare_command_edit(
                CommandEditRequest::ResumeSaved {
                    id: "old-nvenc".into(),
                },
                cancel(),
            )
            .await
            .unwrap_err(),
    );
    unsupported(
        manager
            .av1an_recovery_action(Av1anRecoveryAction::ResumeSaved {
                id: "old-nvenc".into(),
            })
            .await
            .unwrap_err(),
    );
    unsupported(
        manager
            .av1an_recovery_action(Av1anRecoveryAction::ResumeNew {
                id: "old-nvenc".into(),
                settings: Box::new(settings.clone()),
            })
            .await
            .unwrap_err(),
    );
    unsupported(
        manager
            .prepare_command_edit(
                CommandEditRequest::ResumeNew {
                    id: "old-nvenc".into(),
                    settings: Box::new(settings),
                },
                cancel(),
            )
            .await
            .unwrap_err(),
    );
    assert_eq!(manager.list_jobs().await, before);
    manager.shutdown().await;
    let saved: serde_json::Value =
        serde_json::from_slice(&fs::read(history.join("jobs.json")).unwrap()).unwrap();
    assert_eq!(saved["jobs"][0]["encodeSettings"]["encoder"], "h264Nvenc");
}
async fn terminal(manager: &JobManager, id: &str) -> JobSnapshot {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == id)
                .unwrap();
            if job.state.is_terminal() {
                return job;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe"]
async fn copy_drop_and_batch_ignore_unused_hardware_selector_without_reencoding_imported_media() {
    let fixture = Fixture::new();
    let request = fixture.request(VideoEncoder::H264Nvenc);
    assert!(
        std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "color=size=64x48:rate=4:duration=1",
                "-f",
                "lavfi",
                "-i",
                "sine=duration=1",
                "-c:v",
                "ffv1",
                "-threads",
                "1",
                "-c:a",
                "flac",
                "-metadata:s:v:0",
                "encoder=Hardware-origin imported media"
            ])
            .arg(&request.source.input_path)
            .status()
            .unwrap()
            .success()
    );
    let manager = JobManager::new(fixture.0.join("logs"));
    for (index, mode) in [VideoMode::Copy, VideoMode::Drop].into_iter().enumerate() {
        let mut request = request.clone();
        request.source.output_path = fixture
            .0
            .join(format!("inactive-{index}.mkv"))
            .to_string_lossy()
            .into_owned();
        request.source.stream_indices = if mode == VideoMode::Copy {
            vec![0, 1]
        } else {
            vec![1]
        };
        request.settings.video_mode = mode;
        request.settings.source_nvenc_quality = true;
        let admitted = manager.start_encode(request.clone()).await.unwrap();
        let finished = terminal(&manager, &admitted.id).await;
        assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
        let media = media_runtime::probe_media(request.source.output_path)
            .await
            .unwrap();
        assert_eq!(
            media
                .streams
                .iter()
                .filter(|stream| stream.kind == "video")
                .count(),
            usize::from(mode == VideoMode::Copy)
        );
        if mode == VideoMode::Copy {
            assert_eq!(media.streams[0].codec.as_deref(), Some("ffv1"));
        }
    }
    let mut request = batch(&request);
    request.output_pixel_format = Some(Av1anPixelFormat::Yuv420p10le);
    request.source_nvenc_quality = true;
    request.inputs[0].video_mode = Some(VideoMode::Drop);
    request.inputs[0].stream_indices = vec![1];
    let preview = manager.preview_encode_batch(request).await.unwrap();
    assert!(preview.items[0].error.is_none(), "{:?}", preview.items[0]);
    assert_eq!(
        preview.items[0]
            .request
            .as_ref()
            .unwrap()
            .settings
            .video_mode,
        VideoMode::Drop
    );
    manager.shutdown().await;
}
