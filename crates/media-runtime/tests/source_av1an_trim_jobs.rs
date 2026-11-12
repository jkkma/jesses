//! Actual source AV1AN copied input, metadata, encoded output and owned lifetime.
use media_core::*;
use media_runtime::JobManager;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
#[path = "common/source_trim_fixture.rs"]
mod fixture;
use fixture::*;
fn request_av1an(input: &Path, output: &Path) -> EncodeRequest {
    let trim = serde_json::from_value(json!({"startFrame":24,"endFrameExclusive":72})).unwrap();
    let mut req = request(input, output, trim);
    req.source.stream_indices = vec![2, 0, 1];
    req.settings.encoder = VideoEncoder::SvtAv1;
    req.settings.backend = EncodeBackend::Av1an;
    req.settings.workers = 1;
    req.settings.preset = 12;
    req.settings.crf = 40;
    req.settings.av1an_options = Some(Av1anOptions {
        chunk_method: Av1anChunkMethod::Select,
        split_method: Av1anSplitMethod::FixedChunks,
        maximum_chunk_frames: 0,
        concat_method: Av1anConcatMethod::Ffmpeg,
        pixel_format: Some(Av1anPixelFormat::Yuv420p),
        ..Default::default()
    });
    req
}
fn cut_paths(f: &Fixture) -> Vec<PathBuf> {
    let mut roots = std::fs::read_dir(&f.0)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".source-cut-")
        })
        .collect::<Vec<_>>();
    roots.sort();
    roots
}
fn only_cut(f: &Fixture) -> PathBuf {
    let roots = cut_paths(f);
    assert_eq!(roots.len(), 1);
    std::fs::read_dir(&roots[0])
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("cut.")
        })
        .unwrap()
}
fn compare_packets(a: &Value, b: &Value) {
    for (index, stream) in a["streams"].as_array().unwrap().iter().enumerate() {
        assert_eq!(stream["codec_type"], b["streams"][index]["codec_type"]);
        assert_eq!(stream["codec_name"], b["streams"][index]["codec_name"]);
        let a = packets(a, index as u64);
        let b = packets(b, index as u64);
        assert_eq!(a.len(), b.len());
        for (a, b) in a.iter().zip(b) {
            assert_eq!(a["data_hash"], b["data_hash"]);
            for key in ["pts_time", "dts_time", "duration_time"] {
                if a[key].is_string() && b[key].is_string() {
                    assert!(
                        (number(a, key) - number(b, key)).abs() < 0.0021,
                        "{key}: {a:?} {b:?}"
                    );
                } else {
                    assert_eq!(a[key], b[key]);
                }
            }
        }
    }
}
#[tokio::test]
#[ignore = "requires qualified source SVT-HDR, AV1AN, R79 and FFmpeg9"]
async fn source_av1an_cut_precedes_review_keeps_requested_duration_preroll_and_original_authority()
{
    let f = Fixture::new();
    let base = fixture(&f);
    let metadata = f.0.join("source.ffmeta");
    std::fs::write(&metadata,";FFMETADATA1\ntitle=Original title\nlanguage=jpn\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=1500\ntitle=Opening\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=1500\nEND=4000\ntitle=Rest\n").unwrap();
    let font = f.0.join("font.ttf");
    std::fs::write(&font, b"owned attachment fixture").unwrap();
    let input = f.0.join("original titled source.mkv");
    let mut prepare = args(&["-v", "error", "-i"]);
    prepare.push(base.into());
    prepare.extend(args(&["-f", "ffmetadata", "-i"]));
    prepare.push(metadata.into());
    prepare.extend(args(&[
        "-map",
        "0",
        "-c",
        "copy",
        "-map_metadata",
        "1",
        "-map_chapters",
        "1",
        "-attach",
    ]));
    prepare.push(font.into());
    prepare.extend(args(&[
        "-metadata:s:t:0",
        "mimetype=application/x-truetype-font",
    ]));
    prepare.push(input.clone().into());
    run("ffmpeg", prepare);
    let original = std::fs::read(&input).unwrap();
    let output = f.0.join("user-original-stem.mkv");
    let req = request_av1an(&input, &output);
    let manager = JobManager::new(f.0.join("logs"));
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    let review = manager
        .prepare_command_edit(
            CommandEditRequest::Encode {
                request: Box::new(req.clone()),
            },
            cancel.clone(),
        )
        .await
        .unwrap();
    let prepared = review.job.as_ref().unwrap();
    let raw_copy_log = std::fs::read_to_string(
        f.0.join("logs")
            .join(format!("{}.source-cut.log", prepared.id)),
    )
    .unwrap();
    assert!(raw_copy_log.contains("[stdout] progress=end"));
    assert!(
        prepared
            .logs
            .iter()
            .all(|line| !line.starts_with("progress=") && !line.starts_with("out_time_")),
        "FFmpeg progress belongs in the source-cut log, not the preparation activity: {:?}",
        prepared.logs
    );
    assert!(
        prepared
            .logs
            .iter()
            .any(|line| line == "Verifying the stream-copied AV1AN section."),
        "source-cut packet verification should report its own preparation step"
    );
    assert_eq!(
        prepared.progress_seconds, None,
        "the source-copy clock must not carry into the reviewed AV1AN encode"
    );
    let cut = only_cut(&f);
    let actual = probe(&cut);
    assert_eq!(
        packets(&actual, 0).len(),
        72,
        "source preroll is retained, rather than exact48-frame preprocessing"
    );
    assert_eq!(
        review.job.as_ref().unwrap().request.input_path,
        req.source.input_path
    );
    assert_eq!(review.output_path, req.source.output_path);
    assert_eq!(
        review
            .job
            .as_ref()
            .unwrap()
            .encode_settings
            .as_ref()
            .unwrap()
            .trim,
        req.settings.trim
    );
    let reference = f.0.join("source-copy-reference.mkv");
    let mut a = args(&["-v", "error", "-ss", "1.000", "-i"]);
    a.push(input.clone().into());
    a.extend(args(&[
        "-t",
        "2.000",
        "-map",
        "0",
        "-c",
        "copy",
        "-avoid_negative_ts",
        "make_zero",
        "-ignore_unknown",
    ]));
    a.push(reference.clone().into());
    run("ffmpeg", a.clone());
    let expected = probe(&reference);
    compare_packets(&actual, &expected);
    assert_eq!(actual["chapters"], expected["chapters"]);
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
    let result = terminal(&manager, &job.id).await;
    manager.shutdown().await;
    assert_eq!(result.state, JobState::Succeeded, "{result:#?}");
    let encoded = probe(&output);
    assert_eq!(packets(&encoded, 1).len(), 72);
    assert!(
        cut_paths(&f).is_empty(),
        "completed cut variants must be cleaned"
    );
    assert_eq!(std::fs::read(input).unwrap(), original);
    let proof = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/migration-validation/source-av1an-trim-20261101");
    std::fs::create_dir_all(&proof).unwrap();
    std::fs::write(proof.join("source-copy-and-final.json"),serde_json::to_vec_pretty(&json!({"sourceEquivalentArguments":a.iter().map(|value|value.to_string_lossy()).collect::<Vec<_>>(),"actualCut":actual,"referenceCut":expected,"encoded":encoded})).unwrap()).unwrap();
}

fn file_stamp(path: &Path) -> (Vec<u8>, std::time::SystemTime) {
    (
        std::fs::read(path).unwrap(),
        std::fs::metadata(path).unwrap().modified().unwrap(),
    )
}
fn manifest(workspace: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(workspace.join("manifest.json")).unwrap()).unwrap()
}
fn chunks(workspace: &Path) -> Vec<(PathBuf, Vec<u8>, std::time::SystemTime)> {
    let root = workspace.join("chunks/encode");
    if !root.exists() {
        return vec![];
    }
    let mut files = std::fs::read_dir(root)
        .unwrap()
        .map(|entry| {
            let path = entry.unwrap().path();
            let (bytes, mtime) = file_stamp(&path);
            (path, bytes, mtime)
        })
        .collect::<Vec<_>>();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}
fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}
async fn wait_marker(manager: &JobManager, id: &str, marker: &Path) {
    tokio::time::timeout(std::time::Duration::from_secs(45), async {
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == id)
                .unwrap();
            assert!(!job.state.is_terminal(), "{job:#?}");
            if marker.exists()
                && job
                    .recovery
                    .as_ref()
                    .is_some_and(|r| r.completed_frames > 0)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}
async fn prepare_edit(manager: &JobManager, request: CommandEditRequest) -> CommandEditReview {
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    tokio::time::timeout(
        std::time::Duration::from_secs(45),
        manager.prepare_command_edit(request, cancel),
    )
    .await
    .unwrap()
    .unwrap()
}
async fn confirm(manager: &JobManager, review: CommandEditReview, arguments: Option<String>) {
    let (_owner, cancel) = tokio::sync::watch::channel(false);
    manager
        .confirm_command_edit(
            CommandEditConfirmation {
                id: review.id,
                arguments: arguments.unwrap_or(review.arguments),
            },
            cancel,
        )
        .await
        .unwrap();
}
async fn stopped_cut(f: &Fixture) -> (JobManager, EncodeRequest, String, PathBuf, PathBuf) {
    let input = fixture(f);
    let output = f.0.join("resumed.mkv");
    let req = request_av1an(&input, &output);
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    let review = prepare_edit(
        &manager,
        CommandEditRequest::Encode {
            request: Box::new(req.clone()),
        },
    )
    .await;
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
    let marker = f.0.join("after-av1an");
    let hold = f.0.join("hold");
    std::fs::write(&hold, b"hold").unwrap();
    let (av1an, mux) = review.arguments.rsplit_once(" &&\n").unwrap();
    let edited = format!(
        "{av1an} && touch {} && while [ -e {} ]; do sleep 0.02; done && {mux}",
        quoted(&marker),
        quoted(&hold)
    );
    confirm(&manager, review, Some(edited)).await;
    wait_marker(&manager, &id, &marker).await;
    manager.stop_job(id.clone()).await.unwrap();
    assert_eq!(terminal(&manager, &id).await.state, JobState::Stopped);
    assert!(!chunks(&workspace).is_empty());
    std::fs::remove_file(marker).unwrap();
    (manager, req, id, workspace, hold)
}
#[tokio::test]
#[ignore = "requires qualified source SVT-HDR, AV1AN, R79 and FFmpeg9"]
async fn source_cut_saved_resume_reuses_cut_and_encoded_chunks() {
    let f = Fixture::new();
    let (manager, req, id, workspace, hold) = stopped_cut(&f).await;
    let cut = only_cut(&f);
    let cut_stamp = file_stamp(&cut);
    let cached = chunks(&workspace);
    let saved = manifest(&workspace);
    assert!(saved["source_cut_original"].is_object());
    assert_eq!(saved["source_cut_original"]["path"], req.source.input_path);
    assert!(saved["source_cut_rebind_pending"].is_null());
    manager.shutdown().await;
    drop(manager);
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    manager.ready().await.unwrap();
    let review = prepare_edit(&manager, CommandEditRequest::ResumeSaved { id: id.clone() }).await;
    assert!(review.arguments.contains("--resume"));
    assert_eq!(file_stamp(&cut), cut_stamp);
    assert_eq!(chunks(&workspace), cached);
    confirm(&manager, review, None).await;
    wait_marker(&manager, &id, &f.0.join("after-av1an")).await;
    assert_eq!(chunks(&workspace), cached);
    std::fs::remove_file(hold).unwrap();
    let done = terminal(&manager, &id).await;
    assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
    manager.shutdown().await;
    assert!(cut_paths(&f).is_empty());
}
#[tokio::test]
#[ignore = "requires qualified source SVT-HDR, AV1AN, R79 and FFmpeg9"]
async fn source_cut_new_review_cancellation_restores_recipe_and_keeps_deleted_chunks_deleted() {
    let f = Fixture::new();
    let (manager, req, id, workspace, hold) = stopped_cut(&f).await;
    let old_cut = only_cut(&f);
    let old_stamp = file_stamp(&old_cut);
    let old_manifest = manifest(&workspace);
    let mut settings = req.settings.clone();
    settings.trim =
        Some(serde_json::from_value(json!({"startFrame":48,"endFrameExclusive":96})).unwrap());
    let review = prepare_edit(
        &manager,
        CommandEditRequest::ResumeNew {
            id: id.clone(),
            settings: Box::new(settings.clone()),
        },
    )
    .await;
    assert_eq!(cut_paths(&f).len(), 2);
    assert!(chunks(&workspace).is_empty());
    let changed = manifest(&workspace);
    assert!(changed["source_cut_rebind_pending"].is_string());
    assert!(changed["source_cut_committed_recipe"].is_object());
    assert_ne!(changed["source"]["path"], old_manifest["source"]["path"]);
    manager.cancel_command_edit(&review.id).await.unwrap();
    let canceled = terminal(&manager, &id).await;
    assert_eq!(
        canceled.encode_settings,
        Some(req.settings.clone()),
        "{canceled:#?}"
    );
    assert_eq!(file_stamp(&old_cut), old_stamp);
    assert_eq!(cut_paths(&f).len(), 1);
    assert!(chunks(&workspace).is_empty());
    let restored = manifest(&workspace);
    for key in [
        "source",
        "original_source",
        "source_cut_original",
        "tools",
        "params",
        "edited_command",
    ] {
        assert_eq!(restored[key], old_manifest[key], "{key}");
    }
    assert!(restored["completed"].as_object().unwrap().is_empty());
    assert!(restored["source_cut_committed_recipe"].is_null());
    std::fs::remove_file(hold).unwrap();
    let review = prepare_edit(&manager, CommandEditRequest::ResumeSaved { id: id.clone() }).await;
    confirm(&manager, review, None).await;
    let done = terminal(&manager, &id).await;
    assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires qualified source SVT-HDR, AV1AN, R79 and FFmpeg9"]
async fn source_cut_new_trim_commits_only_the_authorized_variant() {
    let f = Fixture::new();
    let (manager, req, id, workspace, _hold) = stopped_cut(&f).await;
    let old_cut = only_cut(&f);
    let old_stamp = file_stamp(&old_cut);
    let mut settings = req.settings.clone();
    settings.trim =
        Some(serde_json::from_value(json!({"startFrame":48,"endFrameExclusive":96})).unwrap());
    let review = prepare_edit(
        &manager,
        CommandEditRequest::ResumeNew {
            id: id.clone(),
            settings: Box::new(settings.clone()),
        },
    )
    .await;
    assert_eq!(cut_paths(&f).len(), 2);
    assert_eq!(file_stamp(&old_cut), old_stamp);
    let staged = manifest(&workspace);
    assert_ne!(staged["source"]["path"], old_cut.to_string_lossy().as_ref());
    assert_eq!(staged["settings"], serde_json::to_value(&settings).unwrap());
    confirm(&manager, review, None).await;
    let done = terminal(&manager, &id).await;
    assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
    assert_eq!(
        packets(&probe(Path::new(&req.source.output_path)), 1).len(),
        48
    );
    manager.shutdown().await;
    assert!(cut_paths(&f).is_empty());
    assert!(!workspace.exists());
}
#[tokio::test]
#[ignore = "requires qualified source SVT-HDR, AV1AN, R79 and FFmpeg9"]
async fn source_cut_interrupted_new_review_restores_committed_recipe() {
    const CHILD_ROOT: &str = "JESSES_SOURCE_CUT_CRASH_ROOT";
    const CHILD_ID: &str = "JESSES_SOURCE_CUT_CRASH_ID";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = PathBuf::from(root);
        let id = std::env::var(CHILD_ID).unwrap();
        let manager = JobManager::open(root.join("logs"), root.join("history")).await;
        manager.ready().await.unwrap();
        let saved = manager
            .list_jobs()
            .await
            .into_iter()
            .find(|job| job.id == id)
            .unwrap();
        let mut settings = saved.encode_settings.unwrap();
        settings.trim =
            Some(serde_json::from_value(json!({"startFrame":48,"endFrameExclusive":96})).unwrap());
        let _review = prepare_edit(
            &manager,
            CommandEditRequest::ResumeNew {
                id,
                settings: Box::new(settings),
            },
        )
        .await;
        std::fs::write(
            root.join("pending-ready"),
            b"prepared cut and unconfirmed journal",
        )
        .unwrap();
        std::future::pending::<()>().await;
        return;
    }
    let f = Fixture::new();
    let (manager, req, id, workspace, hold) = stopped_cut(&f).await;
    let old_cut = only_cut(&f);
    let old_stamp = file_stamp(&old_cut);
    let saved = manifest(&workspace);
    manager.shutdown().await;
    drop(manager);
    let child_log = std::fs::File::create(f.0.join("crash-child.log")).unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "source_cut_interrupted_new_review_restores_committed_recipe",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_ROOT, &f.0)
        .env(CHILD_ID, &id)
        .stdout(child_log.try_clone().unwrap())
        .stderr(child_log)
        .spawn()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(45), async {
        loop {
            assert!(
                child.try_wait().unwrap().is_none(),
                "{}",
                std::fs::read_to_string(f.0.join("crash-child.log")).unwrap()
            );
            if f.0.join("pending-ready").exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(cut_paths(&f).len(), 2);
    assert!(chunks(&workspace).is_empty());
    assert!(manifest(&workspace)["source_cut_committed_recipe"].is_object());
    child.kill().unwrap();
    child.wait().unwrap();
    let manager = JobManager::open(f.0.join("logs"), f.0.join("history")).await;
    manager.ready().await.unwrap();
    let restored = manager
        .list_jobs()
        .await
        .into_iter()
        .find(|job| job.id == id)
        .unwrap();
    assert_eq!(
        restored.encode_settings,
        Some(req.settings.clone()),
        "{restored:#?}"
    );
    assert_eq!(file_stamp(&old_cut), old_stamp);
    assert_eq!(cut_paths(&f).len(), 1);
    let now = manifest(&workspace);
    for key in [
        "source",
        "source_cut_original",
        "tools",
        "params",
        "edited_command",
    ] {
        assert_eq!(now[key], saved[key], "{key}");
    }
    assert!(chunks(&workspace).is_empty());
    assert!(now["completed"].as_object().unwrap().is_empty());
    assert!(now["source_cut_rebind_pending"].is_null());
    std::fs::remove_file(hold).unwrap();
    let review = prepare_edit(&manager, CommandEditRequest::ResumeSaved { id: id.clone() }).await;
    confirm(&manager, review, None).await;
    let done = terminal(&manager, &id).await;
    assert_eq!(done.state, JobState::Succeeded, "{done:#?}");
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires qualified source SVT-HDR, AV1AN, R79 and FFmpeg9"]
async fn source_cut_bframes_nonzero_origin_and_copied_pgs_match_fork_command() {
    for pgs in [false, true] {
        let f = Fixture::new();
        let base = fixture(&f);
        let input = f.0.join("bframes-original.mkv");
        let mut a = args(&["-v", "error", "-i"]);
        a.push(base.into());
        if pgs {
            a.push("-i".into());
            a.push(pgs_fixture(&f.0).into());
        }
        a.extend(args(&[
            "-map",
            "0:v",
            "-map",
            "0:a",
            "-map",
            if pgs { "1:s" } else { "0:s" },
            "-c:v",
            "libx264",
            "-preset",
            "medium",
            "-x264-params",
            "bframes=3:keyint=48:min-keyint=48:scenecut=0",
            "-c:a",
            "copy",
            "-c:s",
            "copy",
            "-output_ts_offset",
            "5",
        ]));
        a.push(input.clone().into());
        run("ffmpeg", a);
        let output = f.0.join("bframes-final.mkv");
        let req = request_av1an(&input, &output);
        let manager = JobManager::new(f.0.join("logs"));
        let review = prepare_edit(
            &manager,
            CommandEditRequest::Encode {
                request: Box::new(req),
            },
        )
        .await;
        let cut = only_cut(&f);
        let actual = probe(&cut);
        let reference = f.0.join("reference.mkv");
        let mut a = args(&["-v", "error", "-ss", "1.000", "-i"]);
        a.push(input.into());
        a.extend(args(&[
            "-t",
            "2.000",
            "-map",
            "0",
            "-c",
            "copy",
            "-avoid_negative_ts",
            "make_zero",
            "-ignore_unknown",
        ]));
        a.push(reference.clone().into());
        run("ffmpeg", a.clone());
        let expected = probe(&reference);
        compare_packets(&actual, &expected);
        assert!(actual["streams"][0]["has_b_frames"].as_u64().unwrap() > 0);
        let count = packets(&actual, 0).len();
        assert!(count > 48);
        confirm(&manager, review, None).await;
        let id = manager.list_jobs().await[0].id.clone();
        let done = terminal(&manager, &id).await;
        assert_eq!(done.state, JobState::Succeeded, "PGS={pgs}: {done:#?}");
        let encoded = probe(&output);
        assert_eq!(packets(&encoded, 1).len(), count);
        if pgs {
            let copied = packets(&actual, 2);
            let final_packets = packets(&encoded, 0);
            assert_eq!(copied.len(), final_packets.len());
            for (a, b) in copied.iter().zip(final_packets) {
                assert_eq!(a["data_hash"], b["data_hash"]);
                assert!((number(a, "pts_time") - number(b, "pts_time")).abs() < 0.0021);
            }
        }
        let proof = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/migration-validation/source-av1an-trim-20261101");
        std::fs::write(proof.join(format!("bframes-origin-pgs-{pgs}.json")),serde_json::to_vec_pretty(&json!({"arguments":a.iter().map(|v|v.to_string_lossy()).collect::<Vec<_>>(),"cut":actual,"reference":expected,"final":encoded})).unwrap()).unwrap();
        manager.shutdown().await;
    }
}

#[tokio::test]
#[ignore = "requires qualified source SVT-HDR, AV1AN, R79 and FFmpeg9"]
async fn source_cut_saved_and_delete_reject_changed_sources_receipts_and_foreign_files() {
    let f = Fixture::new();
    let (manager, req, id, workspace, _hold) = stopped_cut(&f).await;
    let cut = only_cut(&f);
    let root = cut.parent().unwrap();
    let cached = chunks(&workspace);
    let bytes = std::fs::read(&cut).unwrap();
    let mut changed = bytes.clone();
    let last = changed.len() - 1;
    changed[last] ^= 1;
    std::fs::write(&cut, &changed).unwrap();
    let error = manager.resume_job(id.clone()).await.unwrap_err();
    assert!(
        matches!(
            error.code.as_str(),
            "RECOVERY_INVALID" | "SOURCE_NORMALIZATION_INVALID"
        ),
        "{error:?}"
    );
    assert_eq!(chunks(&workspace), cached);
    std::fs::write(&cut, &bytes).unwrap();
    let receipt = root.join("complete.json");
    let receipt_bytes = std::fs::read(&receipt).unwrap();
    let mut changed: Value = serde_json::from_slice(&receipt_bytes).unwrap();
    changed["cut"]["sha256"] = json!("0".repeat(64));
    std::fs::write(&receipt, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(manager.resume_job(id.clone()).await.is_err());
    assert_eq!(chunks(&workspace), cached);
    std::fs::write(&receipt, &receipt_bytes).unwrap();
    let original = std::fs::read(&req.source.input_path).unwrap();
    let mut altered = original.clone();
    let last = altered.len() - 1;
    altered[last] ^= 1;
    std::fs::write(&req.source.input_path, &altered).unwrap();
    assert_eq!(
        manager.resume_job(id.clone()).await.unwrap_err().code,
        "RECOVERY_INVALID"
    );
    assert_eq!(chunks(&workspace), cached);
    std::fs::write(&req.source.input_path, &original).unwrap();
    let foreign = root.join("unrelated.txt");
    std::fs::write(&foreign, b"preserve this unrelated file").unwrap();
    assert!(
        manager
            .av1an_recovery_action(Av1anRecoveryAction::DeleteEncode { id: id.clone() })
            .await
            .is_err()
    );
    assert!(workspace.exists());
    assert_eq!(
        std::fs::read(&foreign).unwrap(),
        b"preserve this unrelated file"
    );
    assert_eq!(chunks(&workspace), cached);
    std::fs::remove_file(foreign).unwrap();
    manager
        .av1an_recovery_action(Av1anRecoveryAction::DeleteEncode { id })
        .await
        .unwrap();
    assert!(!workspace.exists());
    assert!(cut_paths(&f).is_empty());
    assert_eq!(std::fs::read(&req.source.input_path).unwrap(), original);
    manager.shutdown().await;
}
#[cfg(unix)]
#[tokio::test]
#[ignore = "requires qualified source SVT-HDR, AV1AN, R79, FFmpeg9 and Unix process cancellation"]
async fn source_cut_cancellation_stops_actual_copy_and_removes_only_owned_preparation() {
    use std::os::unix::fs::PermissionsExt;
    const CHILD: &str = "JESSES_SOURCE_CUT_CANCEL_ROOT";
    if let Some(root) = std::env::var_os(CHILD) {
        let root = PathBuf::from(root);
        let existing = std::env::var("JESSES_SOURCE_CUT_CANCEL_ID").ok();
        let manager = JobManager::open(root.join("logs"), root.join("history")).await;
        manager.ready().await.unwrap();
        let action = if let Some(id) = &existing {
            let saved = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == *id)
                .unwrap();
            let mut settings = saved.encode_settings.unwrap();
            settings.trim = Some(
                serde_json::from_value(json!({"startFrame":48,"endFrameExclusive":96})).unwrap(),
            );
            CommandEditRequest::ResumeNew {
                id: id.clone(),
                settings: Box::new(settings),
            }
        } else {
            CommandEditRequest::Encode {
                request: Box::new(request_av1an(
                    &root.join("source.mkv"),
                    &root.join("canceled.mkv"),
                )),
            }
        };
        let (sender, cancel) = tokio::sync::watch::channel(false);
        let worker = manager.clone();
        let task = tokio::spawn(async move { worker.prepare_command_edit(action, cancel).await });
        let pid_path = root.join("copy.pid");
        tokio::time::timeout(std::time::Duration::from_secs(20), async {
            while !pid_path.exists() {
                assert!(!task.is_finished());
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let pid = std::fs::read_to_string(pid_path).unwrap();
        let start = std::time::Instant::now();
        sender.send_replace(true);
        let error = tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert!(
            matches!(
                error.code.as_str(),
                "COMMAND_EDIT_CANCELED" | "JOB_CANCELED"
            ),
            "{error:?}"
        );
        manager.shutdown().await;
        assert!(!Path::new(&format!("/proc/{}", pid.trim())).exists());
        let cut_count = std::fs::read_dir(&root)
            .unwrap()
            .filter(|entry| {
                entry
                    .as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .contains(".source-cut-")
            })
            .count();
        assert_eq!(cut_count, usize::from(existing.is_some()));
        if let Some(id) = existing {
            let restored = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|job| job.id == id)
                .unwrap();
            assert_eq!(
                restored.encode_settings.unwrap().trim,
                Some(
                    serde_json::from_value(json!({"startFrame":24,"endFrameExclusive":72}))
                        .unwrap()
                )
            );
        }
        assert!(!root.join("canceled.mkv").exists());
        std::fs::write(root.join("cancel-result.json"),serde_json::to_vec(&json!({"pid":pid.trim(),"elapsedMilliseconds":start.elapsed().as_millis(),"error":error})).unwrap()).unwrap();
        return;
    }
    for new_settings in [false, true] {
        let f = Fixture::new();
        let existing = if new_settings {
            let (manager, _req, id, _workspace, _hold) = stopped_cut(&f).await;
            manager.shutdown().await;
            Some(id)
        } else {
            fixture(&f);
            None
        };
        let prior = if new_settings {
            Some((only_cut(&f), file_stamp(&only_cut(&f))))
        } else {
            None
        };
        let input = f.0.join("source.mkv");
        let original = std::fs::read(&input).unwrap();
        let wrapper = f.0.join("copy-paced-ffmpeg");
        let actual = PathBuf::from(tool("ffmpeg"));
        let script = format!(
            "#!/bin/bash\nfor value in \"$@\"; do\n if [[ \"$value\" == *-source-cut.partial.* ]]; then\n  echo $$ > {}\n  argv=()\n  for value in \"$@\"; do\n   if [[ \"$value\" == '-i' ]]; then argv+=('-readrate' '0.1'); fi\n   argv+=(\"$value\")\n  done\n  exec {} \"${{argv[@]}}\"\n fi\ndone\nexec {} \"$@\"\n",
            quoted(&f.0.join("copy.pid")),
            quoted(&actual),
            quoted(&actual)
        );
        std::fs::write(&wrapper, script).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        if let Some(id) = existing {
            command.env("JESSES_SOURCE_CUT_CANCEL_ID", id);
        }
        let result = command
            .args([
                "--exact",
                "source_cut_cancellation_stops_actual_copy_and_removes_only_owned_preparation",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD, &f.0)
            .env("JESSES_FFMPEG", wrapper)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(std::fs::read(&input).unwrap(), original);
        let proof = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/migration-validation/source-av1an-trim-20261101");
        std::fs::copy(
            f.0.join("cancel-result.json"),
            proof.join(format!("preparation-cancellation-new-{new_settings}.json")),
        )
        .unwrap();
        if let Some((path, stamp)) = prior {
            assert_eq!(file_stamp(&path), stamp);
            assert_eq!(cut_paths(&f).len(), 1);
        }
    }
}

#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg9, source SVT-HDR, AV1AN and R79"]
async fn source_cut_av1an_automatic_webvtt_and_movtext_conversion_use_verified_cut() {
    for (extension, codec) in [("webm", "webvtt"), ("mp4", "mov_text")] {
        let f = Fixture::new();
        let input = fixture(&f);
        let output = f.0.join(format!("auto.{extension}"));
        let mut req = request_av1an(&input, &output);
        req.source.stream_indices = vec![0, 2];
        let manager = JobManager::new(f.0.join("logs"));
        let review = prepare_edit(
            &manager,
            CommandEditRequest::Encode {
                request: Box::new(req),
            },
        )
        .await;
        let id = review.job.as_ref().unwrap().id.clone();
        let cut = only_cut(&f);
        let retained = f.0.join("independent-cut.mkv");
        std::fs::copy(&cut, &retained).unwrap();
        let cut_probe = probe(&cut);
        confirm(&manager, review, None).await;
        let done = terminal(&manager, &id).await;
        manager.shutdown().await;
        assert_eq!(done.state, JobState::Succeeded, "{extension}: {done:#?}");
        let actual = probe(&output);
        assert_eq!(actual["streams"][1]["codec_name"], codec);
        assert_eq!(packets(&actual, 0).len(), packets(&cut_probe, 0).len());
        let reference = f.0.join(format!("reference.{extension}"));
        let mut a = args(&["-v", "error", "-i"]);
        a.push(output.clone().into());
        a.push("-i".into());
        a.push(retained.into());
        a.extend(args(&[
            "-map", "0:v", "-map", "1:s", "-c", "copy", "-c:s", codec,
        ]));
        a.push(reference.clone().into());
        run("ffmpeg", a);
        let expected = probe(&reference);
        let (copied, expected) = (packets(&actual, 1), packets(&expected, 1));
        assert_eq!(copied.len(), expected.len());
        for (a, b) in copied.iter().zip(expected) {
            assert_eq!(a["data_hash"], b["data_hash"]);
            for key in ["pts_time", "dts_time", "duration_time"] {
                if a[key].is_string() && b[key].is_string() {
                    assert!(
                        (number(a, key) - number(b, key)).abs() < 0.0021,
                        "{key}: {a:?} {b:?}"
                    );
                } else {
                    assert_eq!(a[key], b[key]);
                }
            }
        }
    }
}
