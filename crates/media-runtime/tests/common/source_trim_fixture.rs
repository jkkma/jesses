#![allow(dead_code)]
use media_core::*;
use media_runtime::JobManager;
use serde_json::Value;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
static NEXT: AtomicU64 = AtomicU64::new(0);
pub struct Fixture(pub PathBuf);
impl Fixture {
    pub fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-artifacts/source-trim")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(&root).unwrap();
        Self(std::fs::canonicalize(root).unwrap())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() || std::env::var_os("JESSES_KEEP_TEST_ARTIFACTS").is_some() {
            eprintln!("Retained {}", self.0.display());
        } else {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}
pub fn tool(name: &str) -> OsString {
    std::env::var_os(format!("JESSES_{}", name.to_uppercase())).unwrap_or_else(|| name.into())
}
pub fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(Into::into).collect()
}
pub fn run(name: &str, values: Vec<OsString>) -> Vec<u8> {
    let result = std::process::Command::new(tool(name))
        .args(values)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}
pub fn probe(path: &Path) -> Value {
    let mut a = args(&[
        "-v",
        "error",
        "-show_streams",
        "-show_format",
        "-show_chapters",
        "-show_packets",
        "-show_data_hash",
        "sha256",
        "-of",
        "json",
    ]);
    a.push(path.into());
    serde_json::from_slice(&run("ffprobe", a)).unwrap()
}
pub fn fixture(f: &Fixture) -> PathBuf {
    let srt = f.0.join("captions.ass");
    std::fs::write(&srt,"[Script Info]\nScriptType: v4.00+\n[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\nStyle: Default,Arial,20,&H00FFFFFF,&H000000FF,&H00000000,&H00000000,0,0,0,0,100,100,0,0,1,1,0,2,10,10,10,1\n[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\nDialogue: 0,0:00:00.00,0:00:02.50,Default,,0,0,0,,{\\t(0,1500,\\fs30)}Crossing animated cue\nDialogue: 0,0:00:02.80,0:00:03.50,Default,,0,0,0,,Later\n").unwrap();
    let input = f.0.join("source.mkv");
    let mut a = args(&[
        "-v",
        "error",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=96x64:r=24:d=4",
        "-f",
        "lavfi",
        "-i",
        "sine=frequency=440:sample_rate=48000:duration=4",
        "-i",
    ]);
    a.push(srt.into());
    a.extend(args(&[
        "-map",
        "0:v",
        "-map",
        "1:a",
        "-map",
        "2:s",
        "-c:v",
        "libx264",
        "-preset",
        "ultrafast",
        "-g",
        "48",
        "-c:a",
        "aac",
        "-c:s",
        "copy",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
        "-colorspace",
        "bt709",
        "-chroma_sample_location",
        "left",
    ]));
    a.push(input.clone().into());
    run("ffmpeg", a);
    input
}
pub async fn terminal(manager: &JobManager, id: &str) -> JobSnapshot {
    tokio::time::timeout(Duration::from_secs(60), async {
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
pub fn packets(document: &Value, index: u64) -> Vec<&Value> {
    document["packets"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|packet| packet["stream_index"].as_u64() == Some(index))
        .collect()
}
pub fn number(packet: &Value, key: &str) -> f64 {
    packet[key].as_str().unwrap().parse().unwrap()
}
pub fn request(input: &Path, output: &Path, trim: VideoTrim) -> EncodeRequest {
    EncodeRequest {
        source: RemuxRequest {
            input_path: input.to_string_lossy().into(),
            output_path: output.to_string_lossy().into(),
            stream_indices: vec![2, 0, 1],
        },
        settings: EncodeSettings {
            encoder: VideoEncoder::Vp9,
            backend: EncodeBackend::Standalone,
            source_trim: true,
            trim: Some(trim),
            video_stream_index: 0,
            preset: 4,
            crf: 30,
            ..Default::default()
        },
    }
}

/// Same small authored PGS display set used by image_subtitle_jobs::bitmap_fixture.
/// White 40x20 object on a320x180 canvas, visible from0.25s until1.5s.
pub fn pgs_fixture(root: &Path) -> PathBuf {
    fn segment(bytes: &mut Vec<u8>, pts: u32, kind: u8, payload: &[u8]) {
        bytes.extend(b"PG");
        bytes.extend(pts.to_be_bytes());
        bytes.extend(pts.to_be_bytes());
        bytes.push(kind);
        bytes.extend((payload.len() as u16).to_be_bytes());
        bytes.extend(payload);
    }
    let mut bytes = vec![];
    segment(
        &mut bytes,
        22500,
        0x16,
        &[
            1, 64, 0, 180, 0x10, 0, 0, 0x80, 0, 0, 1, 0, 0, 0, 0, 0, 20, 0, 80,
        ],
    );
    segment(&mut bytes, 22500, 0x17, &[1, 0, 0, 0, 0, 0, 1, 64, 0, 180]);
    segment(
        &mut bytes,
        22500,
        0x14,
        &[0, 0, 0, 16, 128, 128, 0, 1, 235, 128, 128, 255],
    );
    let mut rle = vec![];
    for _ in 0..20 {
        rle.extend([1; 40]);
        rle.extend([0, 0]);
    }
    let mut object = vec![0, 0, 0, 0xc0];
    let length = (rle.len() + 4) as u32;
    object.extend(&length.to_be_bytes()[1..]);
    object.extend([0, 40, 0, 20]);
    object.extend(rle);
    segment(&mut bytes, 22500, 0x15, &object);
    segment(&mut bytes, 22500, 0x80, &[]);
    segment(
        &mut bytes,
        135000,
        0x16,
        &[1, 64, 0, 180, 0x10, 0, 1, 0, 0, 0, 0],
    );
    segment(&mut bytes, 135000, 0x80, &[]);
    let path = root.join("copied-caption.sup");
    std::fs::write(&path, bytes).unwrap();
    path
}
