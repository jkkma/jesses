//! Preserved utility containers, original stream metadata and encoded payloads.
use media_core::{GrainRequest, GrainSource, KeyframeCutRequest, UtilityRequest};
use media_runtime::supervisor::{self, CommandSpec};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::watch;

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "jesses-utility-containers-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("preserved failing fixture {}", self.0.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
fn tool(name: &str) -> PathBuf {
    std::env::var_os(format!("JESSES_{}", name.to_uppercase()))
        .map_or_else(|| PathBuf::from(name), PathBuf::from)
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(Into::into).collect()
}
async fn command(name: &str, values: Vec<OsString>) -> Vec<u8> {
    let (_owner, cancel) = watch::channel(false);
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: tool(name),
            args: values,
            cwd: None,
        },
        cancel,
        4 * 1024 * 1024,
        Duration::from_secs(30),
    )
    .await
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}
async fn document(path: &Path) -> serde_json::Value {
    let mut values = args(&[
        "-v",
        "error",
        "-show_streams",
        "-show_format",
        "-show_chapters",
        "-of",
        "json",
    ]);
    values.push(path.as_os_str().to_owned());
    serde_json::from_slice(&command("ffprobe", values).await).unwrap()
}
async fn packets(path: &Path) -> std::collections::BTreeMap<u64, Vec<String>> {
    let mut values = args(&[
        "-v",
        "error",
        "-show_packets",
        "-show_entries",
        "packet=stream_index,data_hash",
        "-show_data_hash",
        "sha256",
        "-of",
        "json",
    ]);
    values.push(path.as_os_str().to_owned());
    let doc: serde_json::Value = serde_json::from_slice(&command("ffprobe", values).await).unwrap();
    let mut result = std::collections::BTreeMap::<_, Vec<_>>::new();
    for packet in doc["packets"].as_array().unwrap() {
        result
            .entry(packet["stream_index"].as_u64().unwrap())
            .or_default()
            .push(packet["data_hash"].as_str().unwrap().into());
    }
    result
}
async fn base_pixels(path: &Path) -> Vec<String> {
    let mut values = args(&["-v", "error", "-c:v", "libdav1d", "-filmgrain", "0", "-i"]);
    values.push(path.as_os_str().to_owned());
    values.extend(args(&["-map", "0:v:0", "-f", "framemd5", "-"]));
    String::from_utf8(command("ffmpeg", values).await)
        .unwrap()
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| line.rsplit(',').next().unwrap().trim().into())
        .collect()
}
async fn source(path: &Path, video: &str, audio: &str, metadata: Option<&Path>) {
    let mut values = args(&[
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-filter_threads",
        "1",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=128x96:rate=24:duration=2",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=2",
    ]);
    if let Some(metadata) = metadata {
        values.extend(args(&["-f", "ffmetadata", "-i"]));
        values.push(metadata.as_os_str().to_owned());
    }
    values.extend(args(&[
        "-map", "0:v:0", "-map", "1:a:0", "-c:v", video, "-threads", "2", "-g", "12", "-bf", "0",
    ]));
    match video {
        "libaom-av1" => values.extend(args(&["-cpu-used", "8", "-crf", "30", "-b:v", "0"])),
        "libvpx-vp9" => values.extend(args(&["-cpu-used", "8", "-crf", "30", "-b:v", "0"])),
        "libx264" => values.extend(args(&["-preset", "ultrafast"])),
        _ => {}
    }
    values.extend(args(&[
        "-c:a",
        audio,
        "-metadata",
        "title=Original global title",
        "-metadata:s:a:0",
        "title=Original audio title",
        "-metadata:s:a:0",
        "language=jpn",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-color_range",
        "tv",
    ]));
    if metadata.is_some() {
        values.extend(args(&["-map_metadata", "2", "-map_chapters", "2"]));
    }
    if metadata.is_some() && path.extension().is_some_and(|extension| extension == "mkv") {
        let attachment = path.with_file_name("attachment.bin");
        std::fs::write(&attachment, b"complete preserved attachment").unwrap();
        values.push("-attach".into());
        values.push(attachment.as_os_str().to_owned());
        values.extend(args(&[
            "-metadata:s:t",
            "mimetype=application/octet-stream",
            "-metadata:s:t",
            "filename=font.bin",
        ]));
    }
    values.push(path.as_os_str().to_owned());
    command("ffmpeg", values).await;
}

#[tokio::test]
#[ignore = "requires FFmpeg/FFprobe software encoders and grav1synth"]
async fn cut_and_grain_keep_original_containers_packets_metadata_and_base_frames() {
    let fixture = Fixture::new();
    let metadata = fixture.0.join("chapters.txt");
    std::fs::write(&metadata,";FFMETADATA1\ntitle=Original global title\nartist=Original artist\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=0\nEND=1000\ntitle=First\n[CHAPTER]\nTIMEBASE=1/1000\nSTART=1000\nEND=2000\ntitle=Second\n").unwrap();
    let (_owner, cancel) = watch::channel(false);
    for (extension, codec, audio) in [
        ("mkv", "libx264", "aac"),
        ("mp4", "libx264", "aac"),
        ("mov", "libx264", "aac"),
        ("webm", "libvpx-vp9", "libopus"),
        ("avi", "mpeg4", "pcm_s16le"),
        ("ts", "mpeg2video", "mp2"),
    ] {
        let input = fixture.0.join(format!("cut-source.{extension}"));
        source(&input, codec, audio, None).await;
        let original = std::fs::read(&input).unwrap();
        let output = fixture.0.join(format!("cut-output.{extension}"));
        let request = UtilityRequest::KeyframeCut(KeyframeCutRequest {
            input_path: input.to_string_lossy().into_owned(),
            output_path: output.to_string_lossy().into_owned(),
            start_seconds: 0.6,
            end_seconds: 1.5,
        });
        media_runtime::run_utility(request.clone(), cancel.clone())
            .await
            .unwrap_or_else(|cause| panic!("cut {extension}: {cause:#?}"));
        let before = packets(&input).await;
        let after = packets(&output).await;
        for (index, packets) in &after {
            assert!(!packets.is_empty());
            assert!(
                before[index]
                    .windows(packets.len())
                    .any(|slice| slice == packets),
                "cut {extension} stream{index} must copy a complete ordered packet slice"
            );
        }
        let doc = document(&output).await;
        assert_eq!(
            doc["streams"][0]["codec_name"],
            document(&input).await["streams"][0]["codec_name"]
        );
        assert_eq!(std::fs::read(&input).unwrap(), original);
        let bytes = std::fs::read(&output).unwrap();
        assert!(
            media_runtime::run_utility(request, cancel.clone())
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(output).unwrap(), bytes);
        eprintln!("PASS cut {extension}");
    }
    for extension in ["mkv", "mp4", "webm"] {
        let input = fixture.0.join(format!("grain-source.{extension}"));
        source(
            &input,
            "libaom-av1",
            if extension == "webm" {
                "libopus"
            } else {
                "aac"
            },
            Some(&metadata),
        )
        .await;
        let original = std::fs::read(&input).unwrap();
        let original_doc = document(&input).await;
        let original_packets = packets(&input).await;
        let original_pixels = base_pixels(&input).await;
        if extension == "mp4" {
            let unsupported = fixture.0.join("unsupported.mov");
            let error = media_runtime::run_utility(
                UtilityRequest::Grain(GrainRequest::Apply {
                    input_path: input.to_string_lossy().into_owned(),
                    output_path: unsupported.to_string_lossy().into_owned(),
                    source: GrainSource::PhotonNoise {
                        iso: 800,
                        chroma: false,
                    },
                }),
                cancel.clone(),
            )
            .await
            .unwrap_err();
            assert_eq!(error.code, "UTILITY_OUTPUT_FORMAT_UNSUPPORTED");
            assert!(!unsupported.exists());
        }
        let grained = fixture.0.join(format!("grained.{extension}"));
        let apply = UtilityRequest::Grain(GrainRequest::Apply {
            input_path: input.to_string_lossy().into_owned(),
            output_path: grained.to_string_lossy().into_owned(),
            source: GrainSource::PhotonNoise {
                iso: 800,
                chroma: false,
            },
        });
        media_runtime::run_utility(apply.clone(), cancel.clone())
            .await
            .unwrap_or_else(|cause| panic!("grain apply {extension}: {cause:#?}"));
        let rewritten = fixture.0.join(format!("rewritten.{extension}"));
        media_runtime::run_utility(
            UtilityRequest::Grain(GrainRequest::RewriteHeaders {
                input_path: grained.to_string_lossy().into_owned(),
                output_path: rewritten.to_string_lossy().into_owned(),
                source: GrainSource::PhotonNoise {
                    iso: 1600,
                    chroma: false,
                },
            }),
            cancel.clone(),
        )
        .await
        .unwrap_or_else(|cause| panic!("grain rewrite {extension}: {cause:#?}"));
        let first_table = fixture.0.join(format!("first-{extension}.tbl"));
        let second_table = fixture.0.join(format!("rewritten-{extension}.tbl"));
        for (input, table) in [(&grained, &first_table), (&rewritten, &second_table)] {
            media_runtime::run_utility(
                UtilityRequest::Grain(GrainRequest::Extract {
                    input_path: input.to_string_lossy().into_owned(),
                    output_table_path: table.to_string_lossy().into_owned(),
                }),
                cancel.clone(),
            )
            .await
            .unwrap();
        }
        assert_ne!(
            std::fs::read(first_table).unwrap(),
            std::fs::read(second_table).unwrap(),
            "explicit ISO rewrite must change actual synthesis parameters"
        );
        let clean = fixture.0.join(format!("clean.{extension}"));
        media_runtime::run_utility(
            UtilityRequest::Grain(GrainRequest::Remove {
                input_path: grained.to_string_lossy().into_owned(),
                output_path: clean.to_string_lossy().into_owned(),
            }),
            cancel.clone(),
        )
        .await
        .unwrap_or_else(|cause| panic!("grain remove {extension}: {cause:#?}"));
        for output in [&grained, &rewritten, &clean] {
            let doc = document(output).await;
            let chapters = |doc: &serde_json::Value| {
                doc["chapters"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|chapter| {
                        (
                            chapter["start_time"].clone(),
                            chapter["end_time"].clone(),
                            chapter["tags"].clone(),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                chapters(&doc),
                chapters(&original_doc),
                "{extension} chapter times and metadata (independent of integer clock denominator)"
            );
            assert_eq!(
                doc["streams"][1]["tags"]["language"],
                original_doc["streams"][1]["tags"]["language"]
            );
            assert_eq!(
                doc["streams"][1]["tags"]["title"],
                original_doc["streams"][1]["tags"]["title"]
            );
            assert_eq!(
                doc["format"]["tags"]["title"],
                original_doc["format"]["tags"]["title"]
            );
            let actual = packets(output).await;
            for (index, packets) in &original_packets {
                if *index != 0 {
                    assert_eq!(
                        actual.get(index),
                        Some(packets),
                        "{extension} nonvideo stream {index}"
                    );
                }
            }
            assert_eq!(
                base_pixels(output).await,
                original_pixels,
                "{extension} grain edits must not re-encode base frames"
            );
        }
        assert_eq!(std::fs::read(&input).unwrap(), original);
        let bytes = std::fs::read(&grained).unwrap();
        assert!(
            media_runtime::run_utility(apply, cancel.clone())
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(grained).unwrap(), bytes);
        eprintln!("PASS grain {extension}");
    }
}

fn ivf_clock(path: &Path) -> (Vec<u8>, Vec<u64>) {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(&bytes[..4], b"DKIF");
    let header_size = u16::from_le_bytes(bytes[6..8].try_into().unwrap()) as usize;
    let mut offset = header_size;
    let mut ticks = Vec::new();
    while offset < bytes.len() {
        assert!(offset + 12 <= bytes.len());
        let size = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        ticks.push(u64::from_le_bytes(
            bytes[offset + 4..offset + 12].try_into().unwrap(),
        ));
        offset += 12 + size;
    }
    assert_eq!(offset, bytes.len());
    assert_eq!(
        u32::from_le_bytes(bytes[24..28].try_into().unwrap()) as usize,
        ticks.len()
    );
    (bytes[8..28].to_vec(), ticks)
}

#[tokio::test]
#[ignore = "requires the source-pinned grav1synth provider and FFmpeg AV1 encoding"]
async fn source_pinned_ivf_grain_preserves_exact_frame_clock() {
    let fixture = Fixture::new();
    let (_owner, cancel) = watch::channel(false);
    for (name, rate) in [
        ("integer", "24"),
        ("film", "24000/1001"),
        ("ntsc", "30000/1001"),
    ] {
        let input = fixture.0.join(format!("source-{name}.ivf"));
        let filter = format!("testsrc2=size=128x96:rate={rate}:duration=1");
        let mut values = args(&[
            "-v",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "lavfi",
            "-i",
            &filter,
            "-c:v",
            "libaom-av1",
            "-cpu-used",
            "8",
            "-threads",
            "2",
            "-crf",
            "30",
            "-b:v",
            "0",
            "-color_primaries",
            "bt709",
            "-color_trc",
            "bt709",
            "-colorspace",
            "bt709",
            "-color_range",
            "tv",
        ]);
        values.push(input.as_os_str().to_owned());
        command("ffmpeg", values).await;
        let original = std::fs::read(&input).unwrap();
        let original_clock = ivf_clock(&input);
        let original_pixels = base_pixels(&input).await;
        let grained = fixture.0.join(format!("grained-{name}.ivf"));
        let rewritten = fixture.0.join(format!("rewritten-{name}.ivf"));
        let clean = fixture.0.join(format!("clean-{name}.ivf"));
        let mut first_parameters = None;
        for (source, output, rewrite, iso) in [
            (&input, &grained, false, 800),
            (&grained, &rewritten, true, 1600),
        ] {
            let grain = GrainSource::PhotonNoise { iso, chroma: false };
            let request = if rewrite {
                GrainRequest::RewriteHeaders {
                    input_path: source.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    source: grain,
                }
            } else {
                GrainRequest::Apply {
                    input_path: source.to_string_lossy().into_owned(),
                    output_path: output.to_string_lossy().into_owned(),
                    source: grain,
                }
            };
            media_runtime::run_utility(UtilityRequest::Grain(request.clone()), cancel.clone())
                .await
                .unwrap();
            let table = fixture.0.join(format!("{name}-{iso}.tbl"));
            media_runtime::run_utility(
                UtilityRequest::Grain(GrainRequest::Extract {
                    input_path: output.to_string_lossy().into_owned(),
                    output_table_path: table.to_string_lossy().into_owned(),
                }),
                cancel.clone(),
            )
            .await
            .unwrap();
            let parameters =
                media_runtime::read_av1an_grain_table(table.to_string_lossy().into_owned())
                    .unwrap()
                    .lines()
                    .filter(|line| !line.starts_with("E "))
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
            if let Some(first) = &first_parameters {
                assert_ne!(
                    first, &parameters,
                    "ISO rewrite changes synthesis parameters"
                );
            } else {
                first_parameters = Some(parameters);
            }
            let saved = std::fs::read(output).unwrap();
            assert!(
                media_runtime::run_utility(UtilityRequest::Grain(request), cancel.clone())
                    .await
                    .is_err()
            );
            assert_eq!(std::fs::read(output).unwrap(), saved, "no-clobber");
        }
        media_runtime::run_utility(
            UtilityRequest::Grain(GrainRequest::Remove {
                input_path: rewritten.to_string_lossy().into_owned(),
                output_path: clean.to_string_lossy().into_owned(),
            }),
            cancel.clone(),
        )
        .await
        .unwrap();
        for output in [&grained, &rewritten, &clean] {
            assert_eq!(
                ivf_clock(output),
                original_clock,
                "{name}: exact IVF header rational/frame count and every packet timestamp"
            );
            assert_eq!(
                base_pixels(output).await,
                original_pixels,
                "{name}: every decoded base pixel"
            );
        }
        assert_eq!(std::fs::read(input).unwrap(), original);
        eprintln!(
            "PASS IVF apply/rewrite/read/remove {rate},{}frames,exact clock/base pixels/no-clobber",
            original_clock.1.len()
        );
    }
}

#[tokio::test]
#[ignore = "requires the source-pinned grav1synth provider and FFmpeg AV1 encoding"]
async fn every_source_film_stock_applies_to_ivf_with_its_own_embedded_parameters() {
    use sha2::{Digest, Sha256};
    let fixture = Fixture::new();
    let (_owner, cancel) = watch::channel(false);
    let input = fixture.0.join("source.ivf");
    let mut values = args(&[
        "-v",
        "error",
        "-nostdin",
        "-n",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=128x96:rate=24:duration=1",
        "-c:v",
        "libaom-av1",
        "-cpu-used",
        "8",
        "-threads",
        "2",
        "-crf",
        "30",
        "-b:v",
        "0",
    ]);
    values.push(input.as_os_str().to_owned());
    command("ffmpeg", values).await;
    let original = std::fs::read(&input).unwrap();
    let clock = ivf_clock(&input);
    let pixels = base_pixels(&input).await;
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/grav1synth-presets.json")).unwrap();
    for stock in expected["presets"].as_array().unwrap() {
        let name = stock["name"].as_str().unwrap();
        let output = fixture.0.join(format!("{name}.ivf"));
        media_runtime::run_utility(
            UtilityRequest::Grain(GrainRequest::Apply {
                input_path: input.to_string_lossy().into_owned(),
                output_path: output.to_string_lossy().into_owned(),
                source: GrainSource::Preset {
                    preset: name.into(),
                },
            }),
            cancel.clone(),
        )
        .await
        .unwrap_or_else(|cause| panic!("{name}: {cause:#?}"));
        assert_eq!(
            ivf_clock(&output),
            clock,
            "{name}: complete exact IVF clock"
        );
        assert_eq!(
            base_pixels(&output).await,
            pixels,
            "{name}: every base pixel"
        );
        let table = fixture.0.join(format!("{name}.tbl"));
        media_runtime::run_utility(
            UtilityRequest::Grain(GrainRequest::Extract {
                input_path: output.to_string_lossy().into_owned(),
                output_table_path: table.to_string_lossy().into_owned(),
            }),
            cancel.clone(),
        )
        .await
        .unwrap();
        let table =
            media_runtime::read_av1an_grain_table(table.to_string_lossy().into_owned()).unwrap();
        let parameters = table
            .lines()
            .skip_while(|line| !line.starts_with("E "))
            .skip(1)
            .take_while(|line| !line.starts_with("E "))
            .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" ") + "\n")
            .collect::<String>();
        assert_eq!(
            format!("{:x}", Sha256::digest(parameters.as_bytes())),
            stock["firstParametersSha256"].as_str().unwrap(),
            "{name}: source embedded parameters read from actual IVF bitstream"
        );
        eprintln!("PASS film-stock IVF {name}");
    }
    for invalid in ["Super8-3", "16mm-4"] {
        let output = fixture.0.join(format!("invalid-{invalid}.ivf"));
        let cause = media_runtime::run_utility(
            UtilityRequest::Grain(GrainRequest::Apply {
                input_path: input.to_string_lossy().into_owned(),
                output_path: output.to_string_lossy().into_owned(),
                source: GrainSource::Preset {
                    preset: invalid.into(),
                },
            }),
            cancel.clone(),
        )
        .await
        .unwrap_err();
        assert_eq!(cause.code, "UTILITY_GRAIN_PRESET_INVALID");
        assert!(!output.exists());
    }
    assert_eq!(std::fs::read(input).unwrap(), original);
}
