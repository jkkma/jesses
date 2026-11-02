//! Real held-review lifecycle, shell pipelines and publication guards.
use media_core::*;
use media_runtime::JobManager;
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
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-artifacts/source-command-edit");
        fs::create_dir_all(&root).unwrap();
        let path = root.join(format!(
            "fixture-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(fs::canonicalize(path).unwrap())
    }
    fn request(&self, output: &str) -> EncodeRequest {
        EncodeRequest {
            source: RemuxRequest {
                input_path: self
                    .0
                    .join("source ' quoted.mkv")
                    .to_string_lossy()
                    .into_owned(),
                output_path: self.0.join(output).to_string_lossy().into_owned(),
                stream_indices: vec![0],
            },
            settings: EncodeSettings {
                video_mode: VideoMode::Copy,
                ..Default::default()
            },
        }
    }
    fn source(&self) {
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
                    "-c:v",
                    "ffv1",
                    "-threads",
                    "1"
                ])
                .arg(self.request("out.mkv").source.input_path)
                .status()
                .unwrap()
                .success()
        );
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
async fn prepare(manager: &JobManager, request: EncodeRequest) -> CommandEditReview {
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    tokio::time::timeout(
        Duration::from_secs(10),
        manager.prepare_command_edit(
            CommandEditRequest::Encode {
                request: Box::new(request),
            },
            cancel,
        ),
    )
    .await
    .unwrap()
    .unwrap()
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe"]
async fn unchanged_confirm_runs_normal_copy_and_empty_close_cancel_without_publishing_or_saved_override()
 {
    let fixture = Fixture::new();
    fixture.source();
    let original = fs::read(&fixture.request("out.mkv").source.input_path).unwrap();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    manager.ready().await.unwrap();
    let review = prepare(&manager, fixture.request("unchanged.mkv")).await;
    let job = review.job.as_ref().unwrap();
    assert_eq!(job.state, JobState::Preparing);
    assert_eq!(job.edited_command, None);
    assert!(!fixture.0.join("unchanged.mkv").exists());
    assert!(
        fixture
            .0
            .join(format!(".jesses-{}.partial.mkv", job.id))
            .is_file()
    );
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let result = manager
        .confirm_command_edit(
            CommandEditConfirmation {
                id: review.id.clone(),
                arguments: review.arguments,
            },
            cancel.clone(),
        )
        .await
        .unwrap();
    let CommandEditResult::Encode { job } = result else {
        panic!()
    };
    let finished = terminal(&manager, &job.id).await;
    assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
    assert_eq!(finished.edited_command, None);
    assert_eq!(
        manager
            .confirm_command_edit(
                CommandEditConfirmation {
                    id: review.id,
                    arguments: "ignored".into()
                },
                cancel.clone()
            )
            .await
            .unwrap_err()
            .code,
        "COMMAND_EDIT_EXPIRED"
    );
    for (index, clear) in [false, true].into_iter().enumerate() {
        let output = format!("cancel-{index}.mkv");
        let review = prepare(&manager, fixture.request(&output)).await;
        let id = review.job.as_ref().unwrap().id.clone();
        if clear {
            assert_eq!(
                manager
                    .confirm_command_edit(
                        CommandEditConfirmation {
                            id: review.id,
                            arguments: " \n ".into()
                        },
                        cancel.clone()
                    )
                    .await
                    .unwrap_err()
                    .code,
                "COMMAND_EDIT_CANCELED"
            );
        } else {
            manager.cancel_command_edit(&review.id).await.unwrap();
        }
        let finished = terminal(&manager, &id).await;
        assert_eq!(finished.state, JobState::Canceled);
        assert!(finished.edited_command.is_none());
        assert!(!fixture.0.join(output).exists());
        assert!(!fixture.0.join(format!(".jesses-{id}.partial.mkv")).exists());
    }
    assert_eq!(
        fs::read(&fixture.request("out.mkv").source.input_path).unwrap(),
        original
    );
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe and a Unix shell"]
async fn confirmed_pipeline_and_compound_command_change_geometry_and_trim_without_stale_typed_expectations()
 {
    let fixture = Fixture::new();
    fixture.source();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    let review = prepare(&manager, fixture.request("edited.mkv")).await;
    let id = review.job.as_ref().unwrap().id.clone();
    let stage = fixture.0.join(format!(".jesses-{id}.partial.mkv"));
    let marker = fixture.0.join("compound-marker");
    let program = format!(
        "ffmpeg -v error -nostdin -i {} -map 0:v -vf scale=32:24 -t 0.5 -f yuv4mpegpipe - | ffmpeg -v error -nostdin -f yuv4mpegpipe -i - -c:v ffv1 -threads 1 -y {} && printf confirmed > {}",
        quote(&fixture.request("out.mkv").source.input_path),
        quote(&stage.to_string_lossy()),
        quote(&marker.to_string_lossy())
    );
    assert!(!marker.exists());
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let result = manager
        .confirm_command_edit(
            CommandEditConfirmation {
                id: review.id,
                arguments: program.clone(),
            },
            cancel,
        )
        .await
        .unwrap();
    let CommandEditResult::Encode { job } = result else {
        panic!()
    };
    let finished = terminal(&manager, &job.id).await;
    assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
    assert_eq!(finished.edited_command.as_ref().unwrap().arguments, program);
    assert_eq!(fs::read(marker).unwrap(), b"confirmed");
    let actual =
        media_runtime::probe_media(fixture.0.join("edited.mkv").to_string_lossy().into_owned())
            .await
            .unwrap();
    assert_eq!(
        (actual.streams[0].width, actual.streams[0].height),
        (Some(32), Some(24))
    );
    assert!(actual.duration_seconds.unwrap() < 0.6);
    assert!(
        fs::read_to_string(fixture.0.join("history/jobs.json"))
            .unwrap()
            .contains("compound-marker")
    );
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe and a Unix shell"]
async fn held_output_collision_and_upstream_pipeline_failure_never_publish_or_replace_later_owner()
{
    let fixture = Fixture::new();
    fixture.source();
    let manager = JobManager::new(fixture.0.join("logs"));
    let review = prepare(&manager, fixture.request("occupied.mkv")).await;
    let id = review.job.as_ref().unwrap().id.clone();
    fs::write(fixture.0.join("occupied.mkv"), b"later owner").unwrap();
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    manager
        .confirm_command_edit(
            CommandEditConfirmation {
                id: review.id,
                arguments: format!("{} && printf done", review.arguments),
            },
            cancel.clone(),
        )
        .await
        .unwrap();
    let finished = terminal(&manager, &id).await;
    assert_eq!(finished.state, JobState::Failed);
    assert_eq!(finished.error.unwrap().code, "OUTPUT_EXISTS");
    assert_eq!(
        fs::read(fixture.0.join("occupied.mkv")).unwrap(),
        b"later owner"
    );
    let review = prepare(&manager, fixture.request("failed.mkv")).await;
    let id = review.job.as_ref().unwrap().id.clone();
    manager
        .confirm_command_edit(
            CommandEditConfirmation {
                id: review.id,
                arguments: "false | true".into(),
            },
            cancel,
        )
        .await
        .unwrap();
    let finished = terminal(&manager, &id).await;
    assert_eq!(finished.state, JobState::Failed);
    assert_eq!(finished.error.unwrap().code, "EDITED_COMMAND_FAILED");
    assert!(!fixture.0.join("failed.mkv").exists());
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe and x264"]
async fn standalone_review_holds_real_pipeline_and_multipass_and_edited_quality_runs_complete_program()
 {
    let fixture = Fixture::new();
    fixture.source();
    let manager = JobManager::new(fixture.0.join("logs"));
    for (index, two_pass) in [false, true].into_iter().enumerate() {
        let mut request = fixture.request(&format!("standalone-{index}.mkv"));
        request.settings.video_mode = VideoMode::Encode;
        request.settings.encoder = VideoEncoder::X264;
        request.settings.crf = 30;
        if two_pass {
            request.settings.rate_control = Some(VideoRateControl::Bitrate {
                bitrate_kbps: 1500,
                two_pass: true,
            });
        }
        let review = prepare(&manager, request).await;
        assert!(review.arguments.contains("x264"));
        assert!(review.arguments.contains(" | "));
        assert!(review.arguments.contains(" &&\n"));
        assert_eq!(
            review.arguments.matches(" | ").count(),
            if two_pass { 2 } else { 1 }
        );
        let arguments = if two_pass {
            review.arguments
        } else {
            format!(
                "{} && printf changed",
                review.arguments.replace("'--crf' '30'", "'--crf' '22'")
            )
        };
        let (_owner, cancel) = tokio::sync::watch::channel(false);
        let CommandEditResult::Encode { job } = manager
            .confirm_command_edit(
                CommandEditConfirmation {
                    id: review.id,
                    arguments,
                },
                cancel,
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        let finished = terminal(&manager, &job.id).await;
        assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
        assert_eq!(finished.edited_command.is_none(), two_pass);
    }
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe"]
async fn cancel_before_review_id_is_returned_stops_queued_preparation_and_leaves_queue_usable() {
    let fixture = Fixture::new();
    fixture.source();
    let manager = JobManager::new(fixture.0.join("logs"));
    let first = prepare(&manager, fixture.request("first.mkv")).await;
    let second_manager = manager.clone();
    let request = fixture.request("second.mkv");
    let (owner, cancel) = tokio::sync::watch::channel(false);
    let second = tokio::spawn(async move {
        second_manager
            .prepare_command_edit(
                CommandEditRequest::Encode {
                    request: Box::new(request),
                },
                cancel,
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if manager.list_jobs().await.len() == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    owner.send_replace(true);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), second)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .code,
        "COMMAND_EDIT_CANCELED"
    );
    let second_id = manager
        .list_jobs()
        .await
        .into_iter()
        .find(|job| job.request.output_path.ends_with("second.mkv"))
        .unwrap()
        .id;
    assert_eq!(
        terminal(&manager, &second_id).await.state,
        JobState::Canceled
    );
    manager.cancel_command_edit(&first.id).await.unwrap();
    assert_eq!(
        terminal(&manager, &first.job.unwrap().id).await.state,
        JobState::Canceled
    );
    let third = prepare(&manager, fixture.request("third.mkv")).await;
    manager.cancel_command_edit(&third.id).await.unwrap();
    assert_eq!(
        terminal(&manager, &third.job.unwrap().id).await.state,
        JobState::Canceled
    );
    assert!(!fixture.0.join("second.mkv").exists());
    assert!(!fs::read_dir(&fixture.0).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".jesses-")
    }));
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe, pinned AV1AN, R79 runtime and x264"]
async fn av1an_holds_actual_late_program_and_confirmed_encoder_changes_reach_the_output() {
    let fixture = Fixture::new();
    fixture.source();
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    for (index, changed) in [false, true].into_iter().enumerate() {
        let mut request = fixture.request(&format!("av1an-{index}.mkv"));
        request.settings.video_mode = VideoMode::Encode;
        request.settings.encoder = VideoEncoder::X264;
        request.settings.backend = EncodeBackend::Av1an;
        request.settings.crf = 30;
        request.settings.av1an_options = Some(Av1anOptions {
            chunk_method: Av1anChunkMethod::Select,
            split_method: Av1anSplitMethod::FixedChunks,
            maximum_chunk_frames: 2,
            minimum_scene_frames: 1,
            scene_downscale_height: None,
            encoder_threads: Some(1),
            ..Default::default()
        });
        let review = prepare(&manager, request).await;
        assert_eq!(review.executable_label, "av1an");
        assert!(review.arguments.contains("--video-params"));
        assert!(review.arguments.contains(" &&\n"));
        let id = review.job.as_ref().unwrap().id.clone();
        let locator = review.job.as_ref().unwrap().recovery.as_ref().unwrap();
        let manifest_path = PathBuf::from(&locator.workspace).join("manifest.json");
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        assert!(manifest.get("edited_command").is_none());
        let arguments = if changed {
            assert!(review.arguments.contains("--crf 30"));
            review.arguments.replace("--crf 30", "--crf 22")
        } else {
            review.arguments
        };
        let (_owner, cancel) = tokio::sync::watch::channel(false);
        manager
            .confirm_command_edit(
                CommandEditConfirmation {
                    id: review.id,
                    arguments: arguments.clone(),
                },
                cancel,
            )
            .await
            .unwrap();
        let finished = terminal(&manager, &id).await;
        assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
        assert_eq!(
            finished
                .edited_command
                .as_ref()
                .map(|command| command.arguments.as_str()),
            changed.then_some(arguments.as_str())
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let job = manager
                    .list_jobs()
                    .await
                    .into_iter()
                    .find(|job| job.id == id)
                    .unwrap();
                if job.recovery.is_none() {
                    break;
                }
                assert!(job.error.is_none(), "{job:?}");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
    }
    manager.shutdown().await;
}

fn chunk_identity(root: &std::path::Path) -> Vec<(String, std::time::SystemTime, String)> {
    use sha2::{Digest, Sha256};
    let directory = root.join("chunks/encode");
    if !directory.exists() {
        return Vec::new();
    }
    let mut files: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (
                entry.file_name().to_string_lossy().into_owned(),
                entry.metadata().unwrap().modified().unwrap(),
                format!("{:x}", Sha256::digest(fs::read(entry.path()).unwrap())),
            )
        })
        .collect();
    files.sort();
    files
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe, pinned AV1AN, R79 runtime and x264"]
async fn av1an_saved_resume_replays_confirmed_program_and_new_settings_generate_new_arguments() {
    for new_settings in [false, true] {
        let fixture = Fixture::new();
        fixture.source();
        let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
        let mut request = fixture.request("resumed.mkv");
        request.settings.video_mode = VideoMode::Encode;
        request.settings.encoder = VideoEncoder::X264;
        request.settings.backend = EncodeBackend::Av1an;
        request.settings.crf = 30;
        request.settings.av1an_options = Some(Av1anOptions {
            chunk_method: Av1anChunkMethod::Select,
            split_method: Av1anSplitMethod::FixedChunks,
            maximum_chunk_frames: 2,
            minimum_scene_frames: 1,
            scene_downscale_height: None,
            encoder_threads: Some(1),
            ..Default::default()
        });
        let review = prepare(&manager, request.clone()).await;
        let id = review.job.as_ref().unwrap().id.clone();
        let workspace = PathBuf::from(
            &review
                .job
                .as_ref()
                .unwrap()
                .recovery
                .as_ref()
                .unwrap()
                .workspace,
        );
        let reached = fixture.0.join("after-av1an");
        let hold = fixture.0.join("hold-before-mux");
        fs::write(&hold, b"hold").unwrap();
        let (av1an, mux) = review.arguments.rsplit_once(" &&\n").unwrap();
        let edited = format!(
            "{} && touch {} && while [ -e {} ]; do sleep 0.05; done && {mux}",
            av1an.replace("--crf 30", "--crf 22"),
            quote(&reached.to_string_lossy()),
            quote(&hold.to_string_lossy())
        );
        let (_owner, cancel) = tokio::sync::watch::channel(false);
        manager
            .confirm_command_edit(
                CommandEditConfirmation {
                    id: review.id,
                    arguments: edited.clone(),
                },
                cancel.clone(),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let job = manager
                    .list_jobs()
                    .await
                    .into_iter()
                    .find(|job| job.id == id)
                    .unwrap();
                assert!(!job.state.is_terminal(), "{job:?}");
                if reached.exists()
                    && job
                        .recovery
                        .as_ref()
                        .is_some_and(|recovery| recovery.completed_frames > 0)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        manager.stop_job(id.clone()).await.unwrap();
        assert_eq!(terminal(&manager, &id).await.state, JobState::Stopped);
        manager.shutdown().await;
        drop(manager);
        let before = chunk_identity(&workspace);
        assert!(!before.is_empty());
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(workspace.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(manifest["edited_command"]["arguments"], edited);
        assert!(
            manifest["params"]
                .as_array()
                .unwrap()
                .windows(2)
                .any(|pair| pair == [serde_json::json!("--crf"), serde_json::json!("22")])
        );
        fs::remove_file(&reached).unwrap();
        let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
        manager.ready().await.unwrap();
        let canceled_request = if new_settings {
            let mut settings = request.settings.clone();
            settings.crf = 19;
            CommandEditRequest::ResumeNew {
                id: id.clone(),
                settings: Box::new(settings),
            }
        } else {
            CommandEditRequest::ResumeSaved { id: id.clone() }
        };
        let canceled_review = manager
            .prepare_command_edit(canceled_request, cancel.clone())
            .await
            .unwrap();
        manager
            .cancel_command_edit(&canceled_review.id)
            .await
            .unwrap();
        let canceled = terminal(&manager, &id).await;
        assert_eq!(canceled.state, JobState::Canceled);
        assert_eq!(
            canceled.encode_settings.as_ref().unwrap().crf,
            30,
            "An unconfirmed new-settings review must restore saved settings"
        );
        assert_eq!(
            canceled.edited_command.as_ref().unwrap().arguments,
            edited,
            "Closing a review must preserve the previously confirmed actual program"
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if manager
                    .list_av1an_recoveries()
                    .await
                    .unwrap()
                    .iter()
                    .any(|entry| entry.job.id == id)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        if !new_settings {
            assert_eq!(chunk_identity(&workspace), before);
        }
        let resume = if new_settings {
            request.settings.crf = 17;
            CommandEditRequest::ResumeNew {
                id: id.clone(),
                settings: Box::new(request.settings),
            }
        } else {
            CommandEditRequest::ResumeSaved { id: id.clone() }
        };
        let review = tokio::time::timeout(
            Duration::from_secs(15),
            manager.prepare_command_edit(resume, cancel.clone()),
        )
        .await
        .unwrap()
        .unwrap();
        if new_settings {
            assert!(review.arguments.contains("--crf 17"));
            assert!(!review.arguments.contains("while ["));
            assert!(chunk_identity(&workspace).is_empty());
        } else {
            assert!(review.arguments.contains("--crf 22"));
            assert!(review.arguments.contains("--resume"));
            assert!(review.arguments.contains("while ["));
            let still_saved: serde_json::Value =
                serde_json::from_slice(&fs::read(workspace.join("manifest.json")).unwrap())
                    .unwrap();
            assert_eq!(
                still_saved["edited_command"]["arguments"], edited,
                "Preparing a saved review must not persist unconfirmed rebound text"
            );
        }
        manager
            .confirm_command_edit(
                CommandEditConfirmation {
                    id: review.id,
                    arguments: review.arguments,
                },
                cancel,
            )
            .await
            .unwrap();
        if !new_settings {
            tokio::time::timeout(Duration::from_secs(15), async {
                while !reached.exists() {
                    let job = manager
                        .list_jobs()
                        .await
                        .into_iter()
                        .find(|job| job.id == id)
                        .unwrap();
                    assert!(!job.state.is_terminal(), "{job:?}");
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            assert_eq!(
                chunk_identity(&workspace),
                before,
                "Saved-command resume must reuse every verified chunk without rewriting it"
            );
            fs::remove_file(&hold).unwrap();
        }
        let finished = terminal(&manager, &id).await;
        assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
        assert_eq!(finished.edited_command.is_none(), new_settings);
        manager.shutdown().await;
    }
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe, pinned AV1AN, R79 runtime and x264"]
async fn clearing_a_fresh_av1an_review_discards_its_unused_owned_workspace() {
    let fixture = Fixture::new();
    fixture.source();
    let manager = JobManager::new(fixture.0.join("logs"));
    let mut request = fixture.request("canceled-av1an.mkv");
    request.settings.video_mode = VideoMode::Encode;
    request.settings.backend = EncodeBackend::Av1an;
    request.settings.encoder = VideoEncoder::X264;
    request.settings.av1an_options = Some(Av1anOptions {
        chunk_method: Av1anChunkMethod::Select,
        split_method: Av1anSplitMethod::FixedChunks,
        minimum_scene_frames: 1,
        scene_downscale_height: None,
        ..Default::default()
    });
    let review = prepare(&manager, request).await;
    let id = review.job.as_ref().unwrap().id.clone();
    let workspace = PathBuf::from(
        &review
            .job
            .as_ref()
            .unwrap()
            .recovery
            .as_ref()
            .unwrap()
            .workspace,
    );
    assert!(workspace.is_dir());
    manager.cancel_command_edit(&review.id).await.unwrap();
    let finished = terminal(&manager, &id).await;
    assert_eq!(finished.state, JobState::Canceled, "{finished:?}");
    assert!(finished.recovery.is_none());
    assert!(!workspace.exists());
    assert!(!fixture.0.join("canceled-av1an.mkv").exists());
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe"]
async fn subtitle_only_and_external_only_selections_never_reintroduce_unchecked_primary_video() {
    let fixture = Fixture::new();
    fixture.source();
    let subtitle = fixture.0.join("caption.srt");
    fs::write(
        &subtitle,
        b"1\n00:00:00,000 --> 00:00:01,000\nOnly subtitle\n",
    )
    .unwrap();
    let voice = fixture.0.join("voice.wav");
    assert!(
        std::process::Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-nostdin",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:sample_rate=48000:duration=1",
                "-c:a",
                "pcm_s16le"
            ])
            .arg(&voice)
            .status()
            .unwrap()
            .success()
    );
    let manager = JobManager::new(fixture.0.join("logs"));
    for external in [false, true] {
        let mut request = fixture.request(if external {
            "only-external.mkv"
        } else {
            "only-subtitle.mkv"
        });
        request.settings.video_mode = VideoMode::Drop;
        if external {
            request.source.stream_indices.clear();
            request.settings.external_tracks = vec![
                serde_json::from_value(serde_json::json!({"inputPath":voice,"streamIndex":0}))
                    .unwrap(),
            ];
        } else {
            request.source.input_path = subtitle.to_string_lossy().into_owned();
        }
        let review = prepare(&manager, request).await;
        let (_owner, cancel) = tokio::sync::watch::channel(false);
        let CommandEditResult::Encode { job } = manager
            .confirm_command_edit(
                CommandEditConfirmation {
                    id: review.id,
                    arguments: review.arguments,
                },
                cancel,
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        let finished = terminal(&manager, &job.id).await;
        assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
        let media = media_runtime::probe_media(finished.request.output_path)
            .await
            .unwrap();
        assert_eq!(media.streams.len(), 1);
        assert_eq!(
            media.streams[0].kind,
            if external { "audio" } else { "subtitle" }
        );
    }
    let mut invalid = fixture.request("empty-primary-remux.mkv").source;
    invalid.stream_indices.clear();
    assert_eq!(
        manager.start_remux(invalid).await.unwrap_err().code,
        "STREAM_SELECTION_INVALID"
    );
    manager.shutdown().await;
}

#[cfg(unix)]
#[test]
#[ignore = "requires FFmpeg/FFprobe; starts an isolated slow-preparation test process"]
fn cancellation_during_actual_probe_isolated_process() {
    let fixture = Fixture::new();
    let wrapper = fixture.0.join("slow-ffprobe");
    let marker = fixture.0.join("probe-started");
    fs::write(&wrapper, format!("#!/bin/sh\nif [ \"$1\" != '-version' ] && mkdir {} 2>/dev/null; then\n  printf '%s' \"$$\" > {}\n  sleep 30\nfi\nexec /usr/bin/ffprobe \"$@\"\n", quote(&fixture.0.join("probe-once").to_string_lossy()), quote(&marker.to_string_lossy()))).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "cancellation_during_actual_probe_child",
            "--ignored",
            "--nocapture",
        ])
        .env("JESSES_FFPROBE", &wrapper)
        .env("JESSES_COMMAND_PROBE_MARKER", &marker)
        .status()
        .unwrap();
    assert!(status.success());
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "isolated child of cancellation_during_actual_probe_isolated_process"]
async fn cancellation_during_actual_probe_child() {
    let Some(marker) = std::env::var_os("JESSES_COMMAND_PROBE_MARKER").map(PathBuf::from) else {
        return;
    };
    let fixture = Fixture::new();
    fixture.source();
    let manager = JobManager::new(fixture.0.join("logs"));
    let (owner, cancel) = tokio::sync::watch::channel(false);
    let worker = manager.clone();
    let request = fixture.request("blocked.mkv");
    let pending = tokio::spawn(async move {
        worker
            .prepare_command_edit(
                CommandEditRequest::Encode {
                    request: Box::new(request),
                },
                cancel,
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !marker.exists() {
            assert!(!pending.is_finished());
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    owner.send_replace(true);
    let error = tokio::time::timeout(Duration::from_secs(5), pending)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, "COMMAND_EDIT_CANCELED");
    let id = manager.list_jobs().await[0].id.clone();
    assert_eq!(terminal(&manager, &id).await.state, JobState::Canceled);
    let pid: i32 = fs::read_to_string(&marker).unwrap().parse().unwrap();
    assert_eq!(
        unsafe { libc::kill(pid, 0) },
        -1,
        "The blocked FFprobe process survived native cancellation"
    );
    assert!(!fixture.0.join("blocked.mkv").exists());
    let next = prepare(&manager, fixture.request("next.mkv")).await;
    manager.cancel_command_edit(&next.id).await.unwrap();
    assert_eq!(
        terminal(&manager, &next.job.unwrap().id).await.state,
        JobState::Canceled
    );
    manager.shutdown().await;
}
