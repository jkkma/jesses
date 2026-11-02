//! Source Quick Output File row and the two None donor selections.
use media_core::*;
use media_runtime::JobManager;
use std::{
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
static NEXT: AtomicU64 = AtomicU64::new(1);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-artifacts/source-output-metadata")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&p).unwrap();
        Self(fs::canonicalize(p).unwrap())
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
fn tool(name: &str) -> OsString {
    std::env::var_os(format!("JESSES_{}", name.to_uppercase())).unwrap_or_else(|| name.into())
}
fn run(name: &str, args: &[OsString]) -> Vec<u8> {
    let result = std::process::Command::new(tool(name))
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}
fn probe(path: &Path) -> serde_json::Value {
    let mut values = args(&[
        "-v",
        "error",
        "-show_format",
        "-show_streams",
        "-show_chapters",
        "-of",
        "json",
    ]);
    values.push(path.as_os_str().to_owned());
    serde_json::from_slice(&run("ffprobe", &values)).unwrap()
}
fn tag<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    value["tags"]
        .as_object()
        .and_then(|tags| tags.iter().find(|(name, _)| name.eq_ignore_ascii_case(key)))
        .and_then(|(_, value)| value.as_str())
        .filter(|value| !value.is_empty())
}
async fn terminal(manager: &JobManager, id: &str) -> JobSnapshot {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let job = manager
                .list_jobs()
                .await
                .into_iter()
                .find(|j| j.id == id)
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
fn source(fixture: &Fixture, name: &str, title: &str, language: &str, stream_title: &str) {
    let metadata = fixture.0.join(format!("{name}.ffmetadata"));
    fs::write(
        &metadata,
        ";FFMETADATA1\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=500\ntitle=Chapter\n",
    )
    .unwrap();
    let mut values = args(&[
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
        "sine=sample_rate=48000:duration=1",
        "-f",
        "ffmetadata",
        "-i",
    ]);
    values.push(metadata.into_os_string());
    values.extend(args(&[
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-map_metadata",
        "-1",
        "-map_chapters",
        "2",
        "-c:v",
        "ffv1",
        "-c:a",
        "pcm_s16le",
        "-threads",
        "1",
        "-metadata",
    ]));
    values.push(format!("title={title}").into());
    values.extend(args(&["-metadata"]));
    values.push(format!("language={language}").into());
    values.extend(args(&[
        "-metadata",
        "artist=Inherited artist",
        "-metadata:s:v:0",
    ]));
    values.push(format!("title={stream_title}").into());
    values.extend(args(&[
        "-metadata:s:v:0",
        "language=jpn",
        "-metadata:s:a:0",
        "title=Voice",
        "-metadata:s:a:0",
        "language=eng",
    ]));
    let attachment = fixture.0.join("note.txt");
    fs::write(&attachment, "attachment identity").unwrap();
    values.extend(args(&["-attach"]));
    values.push(attachment.into_os_string());
    values.extend(args(&[
        "-metadata:s:t",
        "filename=note.txt",
        "-metadata:s:t",
        "mimetype=text/plain",
    ]));
    values.push(fixture.0.join(name).into_os_string());
    run("ffmpeg", &values);
}
fn request(fixture: &Fixture, name: &str) -> EncodeRequest {
    EncodeRequest {
        source: RemuxRequest {
            input_path: fixture.0.join("source.mkv").to_string_lossy().into_owned(),
            output_path: fixture.0.join(name).to_string_lossy().into_owned(),
            stream_indices: vec![0, 1, 2],
        },
        settings: EncodeSettings {
            video_mode: VideoMode::Copy,
            ..Default::default()
        },
    }
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe and x264"]
async fn source_global_metadata_donor_none_blanks_and_held_programs_execute() {
    let fixture = Fixture::new();
    source(
        &fixture,
        "source.mkv",
        "Source title",
        "source-language",
        "Picture",
    );
    source(
        &fixture,
        "donor.mkv",
        "Donor title",
        "donor-language",
        "Donor picture",
    );
    let original = fs::read(fixture.0.join("source.mkv")).unwrap();
    // Execute the fork's actual None mapping and attachment restoration independently.
    let mut reference = args(&["-v", "error", "-i"]);
    reference.push(fixture.0.join("source.mkv").into_os_string());
    reference.extend(args(&[
        "-map",
        "0",
        "-map_metadata",
        "-1",
        "-map_metadata:s:t",
        "0:s:t",
        "-map_chapters",
        "-1",
        "-c",
        "copy",
    ]));
    reference.push(fixture.0.join("source-none.mkv").into_os_string());
    run("ffmpeg", &reference);
    let reference = probe(&fixture.0.join("source-none.mkv"));
    assert_eq!(tag(&reference["streams"][0], "title"), None);
    assert_eq!(tag(&reference["streams"][1], "language"), None);
    assert_eq!(tag(&reference["streams"][2], "filename"), Some("note.txt"));
    assert_eq!(tag(&reference["format"], "title"), None);
    let manager = JobManager::open(fixture.0.join("logs"), fixture.0.join("history")).await;
    for (index, held, encoded, none, donor, overrides) in [
        (0, false, false, false, false, None),
        (1, false, false, false, true, None),
        (
            2,
            false,
            false,
            false,
            true,
            Some(OutputMetadata {
                title: Some("".into()),
                language: Some("".into()),
            }),
        ),
        (3, false, false, true, false, None),
        (
            4,
            false,
            false,
            true,
            false,
            Some(OutputMetadata {
                title: Some("日本語 — output & % !".into()),
                language: Some("言語".into()),
            }),
        ),
        (
            5,
            false,
            true,
            false,
            true,
            Some(OutputMetadata {
                title: Some("Encoded 日本語".into()),
                language: Some("custom-language".into()),
            }),
        ),
        (
            6,
            true,
            false,
            false,
            true,
            Some(OutputMetadata {
                title: Some("Held 日本語".into()),
                language: Some("jpn".into()),
            }),
        ),
        (
            7,
            true,
            true,
            false,
            true,
            Some(OutputMetadata {
                title: Some("Edited program 日本語".into()),
                language: Some("deu".into()),
            }),
        ),
        (8, false, false, true, false, None),
        (9, false, true, true, false, None),
        (10, false, false, true, false, None),
        (11, false, false, false, false, None),
    ] {
        let name = format!("case-{index}.mkv");
        let mut req = request(&fixture, &name);
        req.settings.output_metadata = overrides.clone();
        if none {
            req.settings.copy_metadata = Some(false);
            req.settings.copy_chapters = Some(false);
        }
        if donor {
            req.settings.metadata_source_path =
                Some(fixture.0.join("donor.mkv").to_string_lossy().into_owned());
            req.settings.chapters_source_path = req.settings.metadata_source_path.clone();
        }
        if encoded {
            req.settings.video_mode = VideoMode::Encode;
            req.settings.encoder = VideoEncoder::X264;
            req.settings.preset = 0;
            req.settings.crf = 28;
        }
        if index == 8 {
            req.settings.track_overrides = vec![EncodeTrackOverride {
                stream_index: 0,
                title: Some("Restored 日本語".into()),
                language: Some("fra".into()),
                default: None,
                forced: None,
            }];
        }
        if index == 10 {
            req.settings.copy_chapters = Some(true);
        }
        if index == 11 {
            req.settings.copy_chapters = Some(false);
        }
        let expected = req.settings.clone();
        let id = if held {
            let (_owner, cancel) = tokio::sync::watch::channel(false);
            let review = manager
                .prepare_command_edit(
                    CommandEditRequest::Encode {
                        request: Box::new(req),
                    },
                    cancel.clone(),
                )
                .await
                .unwrap();
            let program = if index == 7 {
                format!("{} && :", review.arguments)
            } else {
                review.arguments
            };
            let CommandEditResult::Encode { job } = manager
                .confirm_command_edit(
                    CommandEditConfirmation {
                        id: review.id,
                        arguments: program,
                    },
                    cancel,
                )
                .await
                .unwrap()
            else {
                panic!()
            };
            job.id.clone()
        } else {
            manager.start_encode(req).await.unwrap().id
        };
        let result = terminal(&manager, &id).await;
        assert_eq!(result.state, JobState::Succeeded, "case{index}: {result:?}");
        assert_eq!(
            result.encode_settings.as_ref().unwrap().output_metadata,
            expected.output_metadata
        );
        let actual = probe(&fixture.0.join(name));
        let inherited = if donor { "Donor title" } else { "Source title" };
        let language = if donor {
            "donor-language"
        } else {
            "source-language"
        };
        let wanted_title = overrides
            .as_ref()
            .and_then(|v| v.title.as_deref())
            .map_or(if none { None } else { Some(inherited) }, |v| {
                (!v.is_empty()).then_some(v)
            });
        let wanted_language = overrides
            .as_ref()
            .and_then(|v| v.language.as_deref())
            .map_or(if none { None } else { Some(language) }, |v| {
                (!v.is_empty()).then_some(v)
            });
        assert_eq!(tag(&actual["format"], "title"), wanted_title, "case{index}");
        assert_eq!(
            tag(&actual["format"], "language"),
            wanted_language,
            "case{index}"
        );
        assert_eq!(
            tag(&actual["format"], "artist"),
            (!none).then_some("Inherited artist")
        );
        assert_eq!(
            actual["chapters"].as_array().unwrap().len(),
            if expected.copy_chapters == Some(false) {
                0
            } else {
                1
            }
        );
        assert_eq!(
            tag(&actual["streams"][0], "title"),
            if index == 8 {
                Some("Restored 日本語")
            } else {
                (!none).then_some("Picture")
            }
        );
        assert_eq!(
            tag(&actual["streams"][0], "language"),
            if index == 8 {
                Some("fra")
            } else {
                (!none).then_some("jpn")
            }
        );
        assert_eq!(
            tag(&actual["streams"][1], "title"),
            (!none).then_some("Voice")
        );
        assert_eq!(
            tag(&actual["streams"][1], "language"),
            (!none).then_some("eng")
        );
        assert_eq!(tag(&actual["streams"][2], "filename"), Some("note.txt"));
        assert_eq!(tag(&actual["streams"][2], "mimetype"), Some("text/plain"));
    }
    assert_eq!(fs::read(fixture.0.join("source.mkv")).unwrap(), original);
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe"]
async fn batch_none_flags_follow_each_immutable_source_without_a_grid_override() {
    let fixture = Fixture::new();
    source(
        &fixture,
        "source.mkv",
        "First input",
        "first",
        "First picture",
    );
    source(
        &fixture,
        "donor.mkv",
        "Second input",
        "second",
        "Second picture",
    );
    let manager = JobManager::new(fixture.0.join("logs"));
    for copy in [false, true] {
        let inputs = ["source.mkv", "donor.mkv"].map(|name| serde_json::json!({"inputPath":fixture.0.join(name),"streamIndices":[0,1,2],"videoStreamIndex":0}));
        let request:BatchEncodeRequest=serde_json::from_value(serde_json::json!({
            "inputs":inputs,
            "outputDirectory":fixture.0,"outputContainer":"matroska","sourceNaming":{"codec":"Copy","quality":"N/A","preset":"N/A"},"outputNameTemplate":format!("{{name}}-batch-{copy}"),
            "videoMode":"copy","crf":30,"preset":4,"copyMetadata":copy,"copyChapters":false
        })).unwrap();
        let preview = manager.preview_encode_batch(request).await.unwrap();
        for (index, item) in preview.items.into_iter().enumerate() {
            let request = item.request.unwrap();
            assert_eq!(request.settings.copy_metadata, Some(copy));
            assert_eq!(request.settings.copy_chapters, Some(false));
            assert_eq!(request.settings.output_metadata, None);
            assert!(request.settings.track_overrides.is_empty());
            assert!(request.settings.metadata_source_path.is_none());
            let output = PathBuf::from(&request.source.output_path);
            let job = manager.enqueue_encode(request).await.unwrap();
            let finished = terminal(&manager, &job.id).await;
            assert_eq!(finished.state, JobState::Succeeded, "{finished:?}");
            let actual = probe(&output);
            assert_eq!(
                tag(&actual["format"], "title"),
                if copy {
                    Some(if index == 0 {
                        "First input"
                    } else {
                        "Second input"
                    })
                } else {
                    None
                }
            );
            assert!(actual["chapters"].as_array().unwrap().is_empty());
            assert_eq!(
                tag(&actual["streams"][0], "title"),
                if copy {
                    Some(if index == 0 {
                        "First picture"
                    } else {
                        "Second picture"
                    })
                } else {
                    None
                }
            );
            assert_eq!(tag(&actual["streams"][2], "filename"), Some("note.txt"));
        }
    }
    manager.shutdown().await;
}

#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe with Opus and AAC"]
async fn global_metadata_and_none_follow_ogg_and_iso_container_representation() {
    let fixture = Fixture::new();
    let manager = JobManager::new(fixture.0.join("logs"));
    for extension in ["ogg", "m4a", "mp4", "mov"] {
        let source = fixture.0.join(format!("source-{extension}.mka"));
        let mut create = args(&[
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=sample_rate=48000:duration=1",
            "-c:a",
            if extension == "ogg" { "libopus" } else { "aac" },
            "-metadata",
            "title=Original container",
            "-metadata",
            "language=eng",
            "-metadata:s:a:0",
            "title=Original audio",
            "-metadata:s:a:0",
            "language=jpn",
        ]);
        create.push(source.clone().into_os_string());
        run("ffmpeg", &create);
        for (index, copy, override_tags) in [
            (0, true, true),
            (1, false, false),
            (2, false, true),
            (3, true, false),
            (4, true, true),
            (5, false, true),
        ] {
            if index >= 4 && extension != "ogg" {
                continue;
            }
            let title = "Output 日本語 & % !";
            let language = "fra";
            let reference = fixture
                .0
                .join(format!("reference-{extension}-{index}.{extension}"));
            let mut values = args(&["-v", "error", "-i"]);
            values.push(source.clone().into_os_string());
            values.extend(args(&[
                "-map",
                "0:a:0",
                "-map_metadata",
                if copy { "0" } else { "-1" },
                "-map_chapters",
                "-1",
                "-c",
                "copy",
            ]));
            if override_tags {
                values.extend(args(&["-metadata"]));
                values.push(format!("title={title}").into());
                values.extend(args(&["-metadata"]));
                values.push(format!("language={language}").into());
            }
            if index >= 4 {
                values.extend(args(&[
                    "-metadata:s:a:0",
                    "title=Explicit audio 日本語",
                    "-metadata:s:a:0",
                    "language=ita",
                ]));
            }
            values.push(reference.clone().into_os_string());
            run("ffmpeg", &values);
            let reference = probe(&reference);
            let output = fixture
                .0
                .join(format!("native-{extension}-{index}.{extension}"));
            let settings = EncodeSettings {
                video_mode: VideoMode::Drop,
                copy_metadata: Some(copy),
                copy_chapters: Some(false),
                output_metadata: override_tags.then(|| OutputMetadata {
                    title: Some(title.into()),
                    language: Some(language.into()),
                }),
                track_overrides: if index >= 4 {
                    vec![EncodeTrackOverride {
                        stream_index: 0,
                        title: Some("Explicit audio 日本語".into()),
                        language: Some("ita".into()),
                        default: None,
                        forced: None,
                    }]
                } else {
                    Vec::new()
                },
                ..Default::default()
            };
            let job = manager
                .start_encode(EncodeRequest {
                    source: RemuxRequest {
                        input_path: source.to_string_lossy().into_owned(),
                        output_path: output.to_string_lossy().into_owned(),
                        stream_indices: vec![0],
                    },
                    settings,
                })
                .await
                .unwrap();
            let finished = terminal(&manager, &job.id).await;
            assert_eq!(
                finished.state,
                JobState::Succeeded,
                "{extension} case{index}: {finished:?}"
            );
            let actual = probe(&output);
            assert!(actual["chapters"].as_array().unwrap().is_empty());
            if extension == "ogg" {
                // Existing policy preserves global and stream titles independently.
                let expected_title = if override_tags {
                    Some(title)
                } else {
                    copy.then_some("Original container")
                };
                let expected_language = if override_tags {
                    Some(language)
                } else {
                    copy.then_some("eng")
                };
                assert_eq!(
                    tag(&actual["streams"][0], "JESSES_CONTAINER_title"),
                    expected_title
                );
                assert_eq!(
                    tag(&actual["streams"][0], "JESSES_CONTAINER_language"),
                    expected_language
                );
                assert_eq!(
                    tag(&actual["streams"][0], "title"),
                    if index >= 4 {
                        Some("Explicit audio 日本語")
                    } else if copy {
                        Some("Original audio")
                    } else {
                        override_tags.then_some(title)
                    }
                );
                // Copied stream comments take precedence over globals. With
                // None the explicit global row becomes the stream comment.
                assert_eq!(
                    tag(&reference["streams"][0], "title"),
                    if index >= 4 {
                        Some("Explicit audio 日本語")
                    } else if copy {
                        Some("Original audio")
                    } else {
                        override_tags.then_some(title)
                    }
                );
                assert_eq!(
                    tag(&reference["streams"][0], "language"),
                    if index >= 4 {
                        Some("ita")
                    } else if copy {
                        Some("jpn")
                    } else {
                        override_tags.then_some(language)
                    }
                );
                assert_eq!(
                    tag(&actual["streams"][0], "language"),
                    tag(&reference["streams"][0], "language")
                );
            } else {
                // ISO policy uses metadata tags, including a language omitted by
                // the source's default muxer. Title must match both routes.
                assert_eq!(
                    tag(&actual["format"], "title"),
                    tag(&reference["format"], "title")
                );
                assert_eq!(
                    tag(&actual["format"], "language"),
                    if override_tags {
                        Some(language)
                    } else {
                        copy.then_some("eng")
                    }
                );
                assert_eq!(
                    tag(&actual["streams"][0], "handler_name"),
                    if copy {
                        Some("Original audio")
                    } else {
                        Some("SoundHandler")
                    }
                );
            }
        }
    }
    manager.shutdown().await;
}
