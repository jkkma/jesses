//! Actual source-engine concat packet verification, independent CLI construction.
use super::*;
use crate::jobs::discover;
use std::fs;
static TEST_ID: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-artifacts/source-queue-concat")
            .join(format!(
                "{}-{}",
                std::process::id(),
                TEST_ID.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(&path).unwrap();
        Self(path.canonicalize().unwrap())
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
struct Tools {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
    mkvmerge: PathBuf,
}
impl Tools {
    async fn new(cancel: &watch::Receiver<bool>) -> Self {
        Self {
            ffmpeg: discover("ffmpeg", cancel).await.unwrap(),
            ffprobe: discover("ffprobe", cancel).await.unwrap(),
            mkvmerge: discover("mkvmerge", cancel).await.unwrap(),
        }
    }
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(Into::into).collect()
}
async fn run(
    executable: &Path,
    args: Vec<OsString>,
    cwd: Option<PathBuf>,
) -> supervisor::CapturedOutput {
    let (_owner, cancel) = watch::channel(false);
    let spec = CommandSpec {
        executable: executable.to_owned(),
        args,
        cwd,
    };
    let result = supervisor::with_console_profile(ConsoleProfile::Hidden, || {
        supervisor::run_capture(&spec, cancel, 4 * 1024 * 1024, Duration::from_secs(60))
    })
    .await
    .unwrap();
    assert!(
        result.status.success(),
        "{}: {}",
        executable.display(),
        String::from_utf8_lossy(&result.stderr)
    );
    result
}
#[allow(clippy::too_many_arguments)]
async fn chunk(
    tools: &Tools,
    root: &Path,
    index: u32,
    codec: &str,
    color: &str,
    size: &str,
    rate: (u32, u32),
    bframes: bool,
) -> SealedChunk {
    let (extension, format) = match codec {
        "libx264" => ("264", "h264"),
        "libx265" => ("hevc", "hevc"),
        _ => ("ivf", "ivf"),
    };
    let path = root.join(format!("{index:05}.{extension}"));
    let mut a = args(&["-v", "error", "-f", "lavfi", "-i"]);
    a.push(format!("color=c={color}:s={size}:r={}/{}", rate.0, rate.1).into());
    a.extend(args(&[
        "-frames:v",
        "6",
        "-c:v",
        codec,
        "-threads",
        "1",
        "-pix_fmt",
        "yuv420p",
        "-color_range",
        "tv",
        "-colorspace",
        "bt709",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-chroma_sample_location",
        "left",
    ]));
    match codec {
        "libx264" => {
            a.extend(args(&["-preset", "ultrafast", "-x264-params"]));
            a.push(
                format!(
                    "stitchable=1:bframes={}:keyint=6:min-keyint=6:scenecut=0",
                    if bframes { 2 } else { 0 }
                )
                .into(),
            );
        }
        "libx265" => {
            a.extend(args(&["-preset","ultrafast","-x265-params","pools=1:frame-threads=1:bframes=2:keyint=6:min-keyint=6:scenecut=0:log-level=error"]));
        }
        "libvpx-vp9" => a.extend(args(&[
            "-deadline",
            "realtime",
            "-cpu-used",
            "8",
            "-g",
            "6",
            "-crf",
            "35",
            "-b:v",
            "0",
        ])),
        "libaom-av1" => a.extend(args(&[
            "-cpu-used",
            "8",
            "-g",
            "6",
            "-crf",
            "40",
            "-b:v",
            "0",
        ])),
        _ => unreachable!(),
    }
    a.extend(args(&["-f", format]));
    a.push(path.as_os_str().to_owned());
    run(&tools.ffmpeg, a, None).await;
    let bytes = fs::read(&path).unwrap();
    SealedChunk {
        path,
        bytes: bytes.len() as u64,
        sha256: Sha256::digest(&bytes).into(),
        source_frames: 6,
        decoded_frames: 6,
    }
}
async fn joined(
    tools: &Tools,
    f: &Fixture,
    chunks: &[SealedChunk],
    method: Av1anConcatMethod,
    rate: (u32, u32),
    force: bool,
    label: &str,
) -> Temporary {
    let ivf = method == Av1anConcatMethod::Ffmpeg && chunks[0].path.extension().unwrap() == "ivf";
    let mut output = Temporary::create_extension(
        &f.0.join("not-published.mkv"),
        label,
        if ivf { "ivf" } else { "mkv" },
    )
    .unwrap();
    output.close_for_path_writer().unwrap();
    match method {
        Av1anConcatMethod::Mkvmerge => {
            let mut a = args(&["-o"]);
            a.push(output.path.as_os_str().to_owned());
            if force {
                a.push("--default-duration".into());
                a.push(format!("0:{}/{}fps", rate.0, rate.1).into());
            }
            a.push("[".into());
            a.extend(chunks.iter().map(|chunk| chunk.path.as_os_str().to_owned()));
            a.push("]".into());
            run(&tools.mkvmerge, a, None).await;
        }
        Av1anConcatMethod::Ffmpeg => {
            // Independent source CLI: relative quoted names, while the helper
            // uses the engine's escaped absolute-path manifest construction.
            let manifest = f.0.join(format!("{label}.concat"));
            let mut text = String::new();
            for chunk in chunks {
                text.push_str(&format!(
                    "file '{}'\n",
                    chunk.path.file_name().unwrap().to_string_lossy()
                ));
            }
            fs::write(&manifest, text).unwrap();
            let mut a = args(&[
                "-y",
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
            ]);
            // FFmpeg's concat demuxer on Windows cannot resolve relative
            // entries against a verbatim \\?\ manifest pathname.
            a.push(manifest.file_name().unwrap().to_owned());
            a.extend(args(&["-map", "0", "-c", "copy"]));
            a.push(output.path.as_os_str().to_owned());
            run(&tools.ffmpeg, a, Some(f.0.clone())).await;
        }
    }
    output.reopen_after_path_writer().unwrap();
    output.flush_nonempty_async().await.unwrap();
    output
}
fn no_scratch(f: &Fixture) {
    assert!(
        !fs::read_dir(&f.0).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".jesses-source-queue-proof-")),
        "unreleased proof artifact: {}",
        f.0.display()
    );
}
fn method_name(method: Av1anConcatMethod) -> &'static str {
    match method {
        Av1anConcatMethod::Mkvmerge => "mkvmerge",
        Av1anConcatMethod::Ffmpeg => "ffmpeg",
    }
}

#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg/FFprobe x264/x265/VP9/AOM and mkvmerge"]
async fn sealed_chunks_match_source_selected_concat_methods_and_reference_clocks() {
    let (_owner, cancel) = watch::channel(false);
    let tools = Tools::new(&cancel).await;
    for (codec, method, rate, force, mixed, bframes) in [
        (
            "libx264",
            Av1anConcatMethod::Mkvmerge,
            (24, 1),
            true,
            false,
            true,
        ),
        (
            "libx264",
            Av1anConcatMethod::Ffmpeg,
            (24, 1),
            true,
            true,
            false,
        ),
        (
            "libx265",
            Av1anConcatMethod::Mkvmerge,
            (24, 1),
            true,
            false,
            true,
        ),
        (
            "libvpx-vp9",
            Av1anConcatMethod::Mkvmerge,
            (24000, 1001),
            true,
            false,
            false,
        ),
        (
            "libvpx-vp9",
            Av1anConcatMethod::Ffmpeg,
            (24000, 1001),
            true,
            false,
            false,
        ),
        (
            "libaom-av1",
            Av1anConcatMethod::Ffmpeg,
            (24, 1),
            true,
            false,
            false,
        ),
        (
            "libaom-av1",
            Av1anConcatMethod::Mkvmerge,
            (24, 1),
            true,
            false,
            false,
        ),
    ] {
        let f = Fixture::new();
        let encoded_rate = if force { rate } else { (24, 1) };
        let a = chunk(
            &tools,
            &f.0,
            0,
            codec,
            "red",
            "64x48",
            encoded_rate,
            bframes,
        )
        .await;
        let mut b = chunk(
            &tools,
            &f.0,
            1,
            codec,
            "blue",
            if mixed { "80x64" } else { "64x48" },
            encoded_rate,
            bframes,
        )
        .await;
        b.source_frames = 9; // A frozen source ordinal count is not an encoded count.
        let chunks = vec![a, b];
        let actual = joined(&tools, &f, &chunks, method, rate, force, "actual").await;
        let before = fs::read(&actual.path).unwrap();
        let (header, clock) = verify_with_clock(
            &tools.ffmpeg,
            &tools.ffprobe,
            Some(&tools.mkvmerge),
            &actual,
            &chunks,
            method,
            rate,
            force,
            &cancel,
        )
        .await
        .unwrap_or_else(|cause| panic!("{codec} {method:?}: {cause:?}"));
        assert_eq!(clock.frames, 12);
        assert_eq!(clock.origin_ticks, 0);
        assert!(clock.time_base.0 > 0 && clock.time_base.1 > 0);
        assert_ne!(clock.relative_pts_sha256, [0; 32]);
        assert!(header.codec_name.is_some());
        assert_eq!(fs::read(&actual.path).unwrap(), before);
        for chunk in &chunks {
            assert_eq!(
                Sha256::digest(fs::read(&chunk.path).unwrap()).as_slice(),
                chunk.sha256.as_slice()
            );
        }
        no_scratch(&f);
        eprintln!(
            "PASS {codec} {} rate={}/{} force={force} mixed={mixed}; decoded={}, source={}",
            method_name(method),
            rate.0,
            rate.1,
            clock.frames,
            chunks.iter().map(|chunk| chunk.source_frames).sum::<u64>()
        );
    }
}

#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg/FFprobe x264 and mkvmerge"]
async fn concat_proof_rejects_changed_payload_missing_reordered_clock_header_and_chunk_receipt() {
    let f = Fixture::new();
    let (_owner, cancel) = watch::channel(false);
    let tools = Tools::new(&cancel).await;
    let chunks = vec![
        chunk(&tools, &f.0, 0, "libx264", "red", "64x48", (24, 1), true).await,
        chunk(&tools, &f.0, 1, "libx264", "blue", "64x48", (24, 1), true).await,
    ];
    let changed = chunk(&tools, &f.0, 2, "libx264", "green", "64x48", (24, 1), true).await;
    let good = joined(
        &tools,
        &f,
        &chunks,
        Av1anConcatMethod::Mkvmerge,
        (24, 1),
        true,
        "good",
    )
    .await;
    verify(
        &tools.ffmpeg,
        &tools.ffprobe,
        Some(&tools.mkvmerge),
        &good,
        &chunks,
        Av1anConcatMethod::Mkvmerge,
        (24, 1),
        true,
        &cancel,
    )
    .await
    .unwrap();
    let foreign = f.0.join("later-owner.keep");
    fs::write(&foreign, b"foreign owner").unwrap();
    for (label, list) in [
        ("changed", vec![chunks[0].clone(), changed]),
        ("missing", vec![chunks[0].clone()]),
        ("reordered", vec![chunks[1].clone(), chunks[0].clone()]),
    ] {
        let actual = joined(
            &tools,
            &f,
            &list,
            Av1anConcatMethod::Mkvmerge,
            (24, 1),
            true,
            label,
        )
        .await;
        let before = fs::read(&actual.path).unwrap();
        assert!(
            verify(
                &tools.ffmpeg,
                &tools.ffprobe,
                Some(&tools.mkvmerge),
                &actual,
                &chunks,
                Av1anConcatMethod::Mkvmerge,
                (24, 1),
                true,
                &cancel
            )
            .await
            .is_err(),
            "accepted {label}"
        );
        assert_eq!(fs::read(&actual.path).unwrap(), before);
        assert_eq!(fs::read(&foreign).unwrap(), b"foreign owner");
        no_scratch(&f);
    }
    for (label, extra) in [
        ("clock", args(&["-itsoffset", "0.125"])),
        ("header", Vec::new()),
    ] {
        let mut actual = Temporary::create(&f.0.join("never-published.mkv"), label).unwrap();
        actual.close_for_path_writer().unwrap();
        let mut a = args(&["-v", "error", "-y", "-copyts"]);
        a.extend(extra);
        a.push("-i".into());
        a.push(good.path.as_os_str().to_owned());
        a.extend(args(&[
            "-map",
            "0:v:0",
            "-c",
            "copy",
            "-avoid_negative_ts",
            "disabled",
        ]));
        if label == "header" {
            a.extend(args(&["-aspect", "16:9"]));
        }
        a.push(actual.path.as_os_str().to_owned());
        run(&tools.ffmpeg, a, None).await;
        actual.reopen_after_path_writer().unwrap();
        let error = verify(
            &tools.ffmpeg,
            &tools.ffprobe,
            Some(&tools.mkvmerge),
            &actual,
            &chunks,
            Av1anConcatMethod::Mkvmerge,
            (24, 1),
            true,
            &cancel,
        )
        .await
        .unwrap_err();
        assert!(
            if label == "clock" {
                error.message.contains("packet") || error.message.contains("timing")
            } else {
                error.message.contains("header")
            },
            "{label}: {error:?}"
        );
        no_scratch(&f);
    }
    // mkvmerge82 exits successfully but appends these raw IVF chunks at
    // the previous last frame, yielding a duplicate boundary. Preserve the
    // source result as an invalid-clock negative, not a mixed-FPS policy.
    let unforced = vec![
        chunk(
            &tools,
            &f.0,
            3,
            "libvpx-vp9",
            "red",
            "64x48",
            (24, 1),
            false,
        )
        .await,
        chunk(
            &tools,
            &f.0,
            4,
            "libvpx-vp9",
            "blue",
            "64x48",
            (48, 1),
            false,
        )
        .await,
    ];
    let invalid_clock = joined(
        &tools,
        &f,
        &unforced,
        Av1anConcatMethod::Mkvmerge,
        (123, 1),
        false,
        "source-unforced",
    )
    .await;
    let clock_error = verify_with_clock(
        &tools.ffmpeg,
        &tools.ffprobe,
        Some(&tools.mkvmerge),
        &invalid_clock,
        &unforced,
        Av1anConcatMethod::Mkvmerge,
        (123, 1),
        false,
        &cancel,
    )
    .await
    .unwrap_err();
    assert!(
        clock_error.message.contains("strictly increasing"),
        "{clock_error:?}"
    );
    no_scratch(&f);
    let original = fs::read(&chunks[1].path).unwrap();
    let mut altered = original.clone();
    altered.extend_from_slice(b"later mutation");
    fs::write(&chunks[1].path, &altered).unwrap();
    let error = verify(
        &tools.ffmpeg,
        &tools.ffprobe,
        Some(&tools.mkvmerge),
        &good,
        &chunks,
        Av1anConcatMethod::Mkvmerge,
        (24, 1),
        true,
        &cancel,
    )
    .await
    .unwrap_err();
    assert!(error.message.contains("sealed byte receipt"));
    assert_eq!(fs::read(&chunks[1].path).unwrap(), altered);
    no_scratch(&f);
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "requires qualified CPU FFmpeg/FFprobe x264, mkvmerge and Unix process pause"]
async fn concat_reference_pause_cancel_joins_tree_and_cleans_only_owned_paths() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let (_owner, cancel) = watch::channel(false);
    let tools = Tools::new(&cancel).await;
    let chunks = vec![
        chunk(&tools, &f.0, 0, "libx264", "red", "64x48", (24, 1), true).await,
        chunk(&tools, &f.0, 1, "libx264", "blue", "64x48", (24, 1), true).await,
    ];
    let actual = joined(
        &tools,
        &f,
        &chunks,
        Av1anConcatMethod::Mkvmerge,
        (24, 1),
        true,
        "guarded",
    )
    .await;
    let before = fs::read(&actual.path).unwrap();
    let script = f.0.join("slow-mkvmerge");
    let heartbeat = f.0.join("heartbeat");
    let escaped = heartbeat.to_string_lossy().replace('\'', "'\\''");
    fs::write(
        &script,
        format!("#!/bin/sh\nwhile :; do printf tick >> '{escaped}'; sleep 0.025; done\n"),
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();
    let (owner, cancel) = watch::channel(false);
    let control = supervisor::PauseControl::default();
    let paused = control.clone();
    let heartbeat_check = heartbeat.clone();
    let task = tokio::spawn(async move {
        supervisor::with_pause(paused, async move {
            let result = verify(
                &tools.ffmpeg,
                &tools.ffprobe,
                Some(&script),
                &actual,
                &chunks,
                Av1anConcatMethod::Mkvmerge,
                (24, 1),
                true,
                &cancel,
            )
            .await;
            assert_eq!(fs::read(&actual.path).unwrap(), before);
            (result, actual, chunks)
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !heartbeat_check.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    control.set_paused(true).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let bytes = fs::metadata(&heartbeat_check).unwrap().len();
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(fs::metadata(&heartbeat_check).unwrap().len(), bytes);
    control.set_paused(false).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while fs::metadata(&heartbeat_check).unwrap().len() == bytes {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    control.set_paused(true).unwrap();
    owner.send_replace(true);
    let (result, _actual, _chunks) = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap_err().code, "JOB_CANCELED");
    let bytes = fs::metadata(&heartbeat_check).unwrap().len();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(fs::metadata(&heartbeat_check).unwrap().len(), bytes);
    no_scratch(&f);
}
