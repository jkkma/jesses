//! Source ColorData checkbox and selected-target replacement proofs.
use media_core::{
    ColorMetadataInspectionRequest, ColorMetadataTransferRequest, UtilityRequest, UtilityResult,
};
use media_runtime::supervisor::{CommandSpec, run_capture};
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
            "jesses-color-test-{}-{}",
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
            eprintln!("Retained {}", self.0.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(Into::into).collect()
}
async fn run(tool: &str, a: Vec<OsString>) -> Vec<u8> {
    let (_owner, cancel) = watch::channel(false);
    let result = run_capture(
        &CommandSpec {
            executable: tool.into(),
            args: a,
            cwd: None,
        },
        cancel,
        4 * 1024 * 1024,
        Duration::from_secs(60),
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
async fn identify(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&run("mkvmerge", vec!["-J".into(), path.as_os_str().to_owned()]).await)
        .unwrap()
}
async fn pixels(path: &Path) -> Vec<u8> {
    let mut a = args(&["-v", "error", "-i"]);
    a.push(path.as_os_str().to_owned());
    a.extend(args(&[
        "-map", "0:v:0", "-pix_fmt", "yuv420p", "-f", "hash", "-hash", "sha256", "-",
    ]));
    run("ffmpeg", a).await
}
async fn fixtures(root: &Path) -> (PathBuf, PathBuf) {
    let raw = root.join("raw.mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=size=128x96:rate=24:duration=0.5",
        "-vf",
        "setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
        "-c:v",
        "ffv1",
        "-threads",
        "2",
    ]);
    a.push(raw.as_os_str().to_owned());
    run("ffmpeg", a).await;
    let donor = root.join("donor.mkv");
    let mut a = args(&["-o"]);
    a.push(donor.as_os_str().to_owned());
    a.extend(args(&[
        "--colour-matrix",
        "0:9",
        "--colour-transfer-characteristics",
        "0:16",
        "--colour-primaries",
        "0:9",
        "--colour-range",
        "0:1",
        "--max-content-light",
        "0:500",
        "--max-frame-light",
        "0:250",
        "--chromaticity-coordinates",
        "0:0.68,0.32,0.265,0.69,0.15,0.06",
        "--white-colour-coordinates",
        "0:0.3127,0.329",
        "--max-luminance",
        "0:2000",
        "--min-luminance",
        "0:0.0001",
    ]));
    a.push(raw.as_os_str().to_owned());
    run("mkvmerge", a).await;
    let target = root.join("target-base.mkv");
    let mut a = args(&["-o"]);
    a.push(target.as_os_str().to_owned());
    a.extend(args(&[
        "--max-content-light",
        "0:99",
        "--max-frame-light",
        "0:44",
    ]));
    a.push(raw.as_os_str().to_owned());
    run("mkvmerge", a).await;
    (donor, target)
}
#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe and mkvmerge"]
async fn color_data_flags_are_independent_and_in_place_preserves_pixels_and_donor() {
    let fixture = Fixture::new();
    let (donor, base) = fixtures(&fixture.0).await;
    let original = std::fs::read(&donor).unwrap();
    let reference = pixels(&base).await;
    let (_owner, cancel) = watch::channel(false);
    let report = media_runtime::read_color_metadata(
        ColorMetadataInspectionRequest {
            input_path: donor.to_string_lossy().into_owned(),
            video_stream_index: 0,
        },
        cancel,
    )
    .await
    .unwrap();
    assert_eq!(report.color["primaries"], "bt2020");
    assert_eq!(report.hdr["max_content"], "500");
    assert!(report.hdr.contains_key("red_x"));
    assert!(report.summary.contains("max_luminance"));
    for (copy_color, copy_hdr) in [(true, false), (false, true), (true, true), (false, false)] {
        let target = fixture
            .0
            .join(format!("target-{copy_color}-{copy_hdr}.mkv"));
        std::fs::copy(&base, &target).unwrap();
        let before = std::fs::read(&target).unwrap();
        let (_owner, cancel) = watch::channel(false);
        let result = media_runtime::run_utility(
            UtilityRequest::ColorMetadataTransfer(ColorMetadataTransferRequest {
                metadata_source_path: donor.to_string_lossy().into_owned(),
                metadata_source_video_stream_index: 0,
                input_path: target.to_string_lossy().into_owned(),
                input_video_stream_index: 0,
                output_path: String::new(),
                copy_color,
                copy_hdr,
                in_place: true,
            }),
            cancel,
        )
        .await
        .unwrap();
        let UtilityResult::Artifact(artifact) = result else {
            panic!("expected artifact")
        };
        assert_eq!(Path::new(&artifact.output_path), target);
        assert_eq!(pixels(&target).await, reference);
        let document = identify(&target).await;
        let properties = &document["tracks"][0]["properties"];
        assert_eq!(
            properties["color_primaries"].as_u64().unwrap_or(1),
            if copy_color { 9 } else { 1 }
        );
        assert_eq!(
            properties["max_content_light"],
            if copy_hdr { 500 } else { 99 }
        );
        if !copy_color && !copy_hdr {
            assert_eq!(std::fs::read(target).unwrap(), before);
        }
        assert_eq!(std::fs::read(&donor).unwrap(), original);
    }
    let no_hdr_target = fixture.0.join("no-hdr-donor.mkv");
    std::fs::copy(&base, &no_hdr_target).unwrap();
    let before = std::fs::read(&no_hdr_target).unwrap();
    let (_owner, cancel) = watch::channel(false);
    media_runtime::run_utility(
        UtilityRequest::ColorMetadataTransfer(ColorMetadataTransferRequest {
            metadata_source_path: fixture.0.join("raw.mkv").to_string_lossy().into_owned(),
            metadata_source_video_stream_index: 0,
            input_path: no_hdr_target.to_string_lossy().into_owned(),
            input_video_stream_index: 0,
            output_path: String::new(),
            copy_color: false,
            copy_hdr: true,
            in_place: true,
        }),
        cancel,
    )
    .await
    .unwrap();
    assert_eq!(std::fs::read(no_hdr_target).unwrap(), before);
    let target = fixture.0.join("cancel.mkv");
    std::fs::copy(&base, &target).unwrap();
    let before = std::fs::read(&target).unwrap();
    let (_owner, cancel) = watch::channel(true);
    assert!(
        media_runtime::run_utility(
            UtilityRequest::ColorMetadataTransfer(ColorMetadataTransferRequest {
                metadata_source_path: donor.to_string_lossy().into_owned(),
                metadata_source_video_stream_index: 0,
                input_path: target.to_string_lossy().into_owned(),
                input_video_stream_index: 0,
                output_path: String::new(),
                copy_color: true,
                copy_hdr: true,
                in_place: true
            }),
            cancel
        )
        .await
        .is_err()
    );
    assert_eq!(std::fs::read(target).unwrap(), before);
}
#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe H264/VP9 and mkvmerge"]
async fn in_place_color_editing_keeps_mp4_mov_and_webm_container_families() {
    let fixture = Fixture::new();
    let (donor, base) = fixtures(&fixture.0).await;
    for extension in ["mp4", "mov", "webm"] {
        let target = fixture.0.join(format!("target.{extension}"));
        let mut a = args(&["-v", "error", "-i"]);
        a.push(base.as_os_str().to_owned());
        a.extend(if extension == "webm" {
            args(&[
                "-c:v",
                "libvpx-vp9",
                "-lossless",
                "1",
                "-deadline",
                "realtime",
                "-cpu-used",
                "8",
            ])
        } else {
            args(&["-c:v", "libx264", "-preset", "ultrafast", "-crf", "0"])
        });
        a.extend(args(&[
            "-metadata",
            "title=Preserved target title",
            "-metadata",
            "comment=Preserved & authored comment",
            "-metadata:s:v:0",
            "language=jpn",
            "-metadata:s:v:0",
            "handler_name=Preserved picture",
        ]));
        a.push(target.as_os_str().to_owned());
        run("ffmpeg", a).await;
        let before = pixels(&target).await;
        let (_owner, cancel) = watch::channel(false);
        let result = media_runtime::run_utility(
            UtilityRequest::ColorMetadataTransfer(ColorMetadataTransferRequest {
                metadata_source_path: donor.to_string_lossy().into_owned(),
                metadata_source_video_stream_index: 0,
                input_path: target.to_string_lossy().into_owned(),
                input_video_stream_index: 0,
                output_path: String::new(),
                copy_color: true,
                copy_hdr: false,
                in_place: true,
            }),
            cancel,
        )
        .await;
        assert!(result.is_ok(), "{extension}: {result:#?}");
        assert_eq!(pixels(&target).await, before);
        let document = identify(&target).await;
        assert_eq!(document["tracks"][0]["properties"]["color_primaries"], 9);
        let bytes = std::fs::read(&target).unwrap();
        if extension == "webm" {
            assert_eq!(&bytes[..4], &[0x1a, 0x45, 0xdf, 0xa3]);
            assert!(
                bytes[..128.min(bytes.len())]
                    .windows(7)
                    .any(|value| value == [0x42, 0x82, 0x84, b'w', b'e', b'b', b'm'])
            );
        } else {
            assert_eq!(&bytes[4..8], b"ftyp");
            assert_eq!(
                &bytes[8..12],
                if extension == "mov" { b"qt  " } else { b"isom" }
            );
        }
        let mut a = args(&[
            "-v",
            "error",
            "-show_entries",
            "format_tags:stream_tags",
            "-of",
            "json",
        ]);
        a.push(target.as_os_str().to_owned());
        let metadata: serde_json::Value = serde_json::from_slice(&run("ffprobe", a).await).unwrap();
        let global = metadata["format"]["tags"].as_object().unwrap();
        for (key, expected) in [
            ("title", "Preserved target title"),
            ("comment", "Preserved & authored comment"),
        ] {
            assert!(
                global
                    .iter()
                    .any(|(name, value)| name.eq_ignore_ascii_case(key) && value == expected),
                "{extension}: {metadata:#?}"
            );
        }
        assert_eq!(metadata["streams"][0]["tags"]["language"], "jpn");
        if extension != "webm" {
            let preserved = std::fs::read(&target).unwrap();
            let (_owner, cancel) = watch::channel(false);
            let result = media_runtime::run_utility(
                UtilityRequest::ColorMetadataTransfer(ColorMetadataTransferRequest {
                    metadata_source_path: donor.to_string_lossy().into_owned(),
                    metadata_source_video_stream_index: 0,
                    input_path: target.to_string_lossy().into_owned(),
                    input_video_stream_index: 0,
                    output_path: String::new(),
                    copy_color: false,
                    copy_hdr: true,
                    in_place: true,
                }),
                cancel,
            )
            .await;
            let error=result.expect_err("H264 ISO containers cannot carry replacement static HDR while their original SEI stays byte-identical");
            assert_eq!(
                error.code, "UTILITY_COLOR_CONTAINER_UNSUPPORTED",
                "{error:#?}"
            );
            assert_eq!(std::fs::read(&target).unwrap(), preserved);
        } else {
            let (_owner, cancel) = watch::channel(false);
            media_runtime::run_utility(
                UtilityRequest::ColorMetadataTransfer(ColorMetadataTransferRequest {
                    metadata_source_path: donor.to_string_lossy().into_owned(),
                    metadata_source_video_stream_index: 0,
                    input_path: target.to_string_lossy().into_owned(),
                    input_video_stream_index: 0,
                    output_path: String::new(),
                    copy_color: false,
                    copy_hdr: true,
                    in_place: true,
                }),
                cancel,
            )
            .await
            .unwrap();
            let document = identify(&target).await;
            assert_eq!(
                document["tracks"][0]["properties"]["max_content_light"],
                500
            );
            assert_eq!(pixels(&target).await, before);
        }
    }
}
#[tokio::test]
#[ignore = "requires qualified FFmpeg/FFprobe and mkvmerge"]
async fn legacy_copy_to_new_remains_no_clobber_and_unsupported_targets_are_unchanged() {
    let fixture = Fixture::new();
    let (donor, input) = fixtures(&fixture.0).await;
    let input_bytes = std::fs::read(&input).unwrap();
    let output = fixture.0.join("new.mkv");
    let request = ColorMetadataTransferRequest {
        metadata_source_path: donor.to_string_lossy().into_owned(),
        metadata_source_video_stream_index: 0,
        input_path: input.to_string_lossy().into_owned(),
        input_video_stream_index: 0,
        output_path: output.to_string_lossy().into_owned(),
        copy_color: true,
        copy_hdr: true,
        in_place: false,
    };
    let (_owner, cancel) = watch::channel(false);
    media_runtime::run_utility(
        UtilityRequest::ColorMetadataTransfer(request.clone()),
        cancel,
    )
    .await
    .unwrap();
    assert_eq!(pixels(&input).await, pixels(&output).await);
    assert_eq!(std::fs::read(&input).unwrap(), input_bytes);
    let published = std::fs::read(&output).unwrap();
    let (_owner, cancel) = watch::channel(false);
    let error = media_runtime::run_utility(
        UtilityRequest::ColorMetadataTransfer(request.clone()),
        cancel,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "OUTPUT_EXISTS");
    assert_eq!(std::fs::read(&output).unwrap(), published);
    #[cfg(unix)]
    {
        let link = fixture.0.join("selected-link.mkv");
        std::os::unix::fs::symlink(&input, &link).unwrap();
        let (_owner, cancel) = watch::channel(false);
        let error = media_runtime::run_utility(
            UtilityRequest::ColorMetadataTransfer(ColorMetadataTransferRequest {
                input_path: link.to_string_lossy().into_owned(),
                in_place: true,
                ..request.clone()
            }),
            cancel,
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, "UTILITY_TARGET_SYMLINK");
        assert_eq!(std::fs::read(&input).unwrap(), input_bytes);
    }
    let unsupported = fixture.0.join("selected.avi");
    std::fs::copy(&input, &unsupported).unwrap();
    let before = std::fs::read(&unsupported).unwrap();
    let (_owner, cancel) = watch::channel(false);
    let error = media_runtime::run_utility(
        UtilityRequest::ColorMetadataTransfer(ColorMetadataTransferRequest {
            input_path: unsupported.to_string_lossy().into_owned(),
            output_path: String::new(),
            in_place: true,
            ..request
        }),
        cancel,
    )
    .await
    .unwrap_err();
    assert_eq!(error.code, "CONTAINER_INCOMPATIBLE");
    assert_eq!(std::fs::read(&unsupported).unwrap(), before);
}
