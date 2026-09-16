//! Validate av1an's executable queue before accepting it as a recovery receipt.
//! This module reads no files and never executes the serialized commands.
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

use media_core::{Av1anChunkMethod, Av1anOptions};

use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};
use sha2::{Digest, Sha256};

const MAX_RECEIPT_BYTES: usize = 16 * 1024 * 1024;
const MAX_CHUNKS: usize = 100_000;
// av1an 0.5.2-unstable (7df934d) generated loadscript, with only the source and
// cache_file assignment values replaced by placeholders and CRLF normalized.
// Fail closed when a different engine changes the executable Python template.
const SOURCE_SCRIPT_SHA256: &str =
    "7339ee66ff9bab9b4e2a3286374070a97975bd3cc622f1239bd44926419a6171";

pub(super) struct Expected<'a> {
    pub source: &'a Path,
    pub chunks_directory: &'a Path,
    pub video_params: &'a [String],
    pub total_frames: u64,
    pub fps_num: u32,
    pub fps_den: u32,
    /// Declared source rate used by av1an's queue metadata. Complete timestamp
    /// validation can establish a different exact output cadence; video_params
    /// still must match that independently validated encoding plan verbatim.
    pub source_fps_num: u32,
    pub source_fps_den: u32,
    /// Bytes from the separately fingerprinted, tool-created loadscript.vpy.
    pub options: Av1anOptions,
    pub script_text: &'a str,
    pub source_filter: Option<&'a str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CompletedChunk {
    pub name: String,
    pub index: usize,
    pub frames: u64,
    pub size_bytes: u64,
}

#[derive(Debug)]
pub(super) struct Receipts {
    pub completed: Vec<CompletedChunk>,
    pub completed_frames: u64,
    pub total_frames: u64,
    pub queued_chunks: usize,
    pub script_path: Option<PathBuf>,
    pub segments: Vec<PathBuf>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Chunk {
    temp: PathBuf,
    index: usize,
    input: Input,
    proxy: Option<serde_json::Value>,
    source_cmd: Vec<OsString>,
    proxy_cmd: Option<serde_json::Value>,
    output_ext: String,
    start_frame: u64,
    end_frame: u64,
    frame_rate: f64,
    passes: u8,
    video_params: Vec<String>,
    encoder: String,
    noise_size: (Option<u32>, Option<u32>),
    target_quality: TargetQuality,
    per_shot_target_quality_cq: Option<f64>,
    ignore_frame_mismatch: bool,
}

// Target quality is disabled for this plan. Still reject unknown engine fields:
// these saved settings are interpreted by av1an during resume.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TargetQuality {
    vmaf_res: String,
    probe_res: Option<(u32, u32)>,
    vmaf_scaler: String,
    vmaf_filter: Option<String>,
    vmaf_threads: usize,
    model: Option<String>,
    probing_rate: u64,
    probes: u64,
    target: Option<(f64, f64)>,
    metric: String,
    min_q: u64,
    max_q: u64,
    interp_method: Option<String>,
    encoder: String,
    pix_format: String,
    temp: PathBuf,
    workers: usize,
    video_params: Option<Vec<String>>,
    params_copied: bool,
    vspipe_args: Vec<String>,
    probing_vmaf_features: Vec<String>,
    probing_statistic: ProbingStatistic,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbingStatistic {
    name: String,
    value: Option<f64>,
}

impl TargetQuality {
    fn matches_plan(&self, expected: &Expected<'_>) -> bool {
        let Some(target) = expected.options.target_quality else {
            return self.matches_disabled_plan(expected.chunks_directory);
        };
        self.vmaf_res == format!("{}x{}", target.probe_width, target.probe_height)
            && self.probe_res
                == Some((
                    u32::from(target.probe_width),
                    u32::from(target.probe_height),
                ))
            && self.vmaf_scaler == "bicubic"
            && self.vmaf_filter.as_deref() == expected.source_filter
            && self.vmaf_threads == 2
            && self.model.is_none()
            && self.probing_rate == u64::from(target.probing_rate)
            && self.probes == u64::from(target.probes)
            && self.target
                == Some((
                    f64::from(target.minimum_score_tenths) / 10.0,
                    f64::from(target.maximum_score_tenths) / 10.0,
                ))
            && self.metric == super::metrics::receipt(target.metric)
            && self.min_q == u64::from(target.minimum_crf)
            && self.max_q == u64::from(target.maximum_crf)
            && self.interp_method.is_none()
            && self.encoder == "svt_av1"
            && self.pix_format == "YUV420P10LE"
            && same_path(&self.temp, expected.chunks_directory)
            && self.workers > 0
            && self.video_params.as_deref() == Some(expected.video_params)
            && !self.params_copied
            && self.vspipe_args.is_empty()
            && self.probing_vmaf_features == ["Default"]
            && self.probing_statistic.name == "Mean"
            && self.probing_statistic.value.is_none()
    }
    fn matches_disabled_plan(&self, chunks: &Path) -> bool {
        self.vmaf_res == "1920x1080"
            && self.probe_res.is_none()
            && self.vmaf_scaler == "bicubic"
            && self.vmaf_filter.is_none()
            && self.vmaf_threads > 0
            && self.model.is_none()
            && self.probing_rate == 1
            && self.probes == 4
            && self.target.is_none()
            && self.metric == "VMAF"
            && self.min_q == 15
            && self.max_q == 50
            && self.interp_method.is_none()
            && self.encoder == "svt_av1"
            && self.pix_format == "YUV420P10LE"
            && same_path(&self.temp, chunks)
            && self.workers > 0
            && self.video_params.is_none()
            && !self.params_copied
            && self.vspipe_args.is_empty()
            && self.probing_vmaf_features == ["Default"]
            && self.probing_statistic.name == "Automatic"
            && self.probing_statistic.value.is_none()
    }
}

#[derive(Deserialize)]
enum Input {
    VapourSynth(Script),
    Video(Video),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Video {
    path: PathBuf,
    temp: PathBuf,
    chunk_method: String,
    is_proxy: bool,
    cache_mode: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Script {
    path: PathBuf,
    vspipe_args: Vec<String>,
    script_text: String,
    is_proxy: bool,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Scene {
    start_frame: u64,
    end_frame: u64,
    zone_overrides: Option<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenes {
    frames: u64,
    scenes: Vec<Scene>,
    split_scenes: Vec<Scene>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Done {
    frames: u64,
    done: DoneMap,
    #[serde(rename = "audio_done")]
    _audio_done: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DoneChunk {
    frames: u64,
    size_bytes: u64,
}

struct DoneMap(BTreeMap<String, DoneChunk>);

impl<'de> Deserialize<'de> for DoneMap {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct UniqueMap;
        impl<'de> Visitor<'de> for UniqueMap {
            type Value = DoneMap;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a map with unique completed chunk names")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
                let mut values = BTreeMap::new();
                while let Some((key, value)) = map.next_entry::<String, DoneChunk>()? {
                    if values.len() >= MAX_CHUNKS || values.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom(
                            "duplicate or excessive completed chunks",
                        ));
                    }
                }
                Ok(DoneMap(values))
            }
        }
        deserializer.deserialize_map(UniqueMap)
    }
}

fn parse<T: for<'de> Deserialize<'de>>(bytes: &[u8], name: &str) -> Result<T, String> {
    if bytes.is_empty() || bytes.len() > MAX_RECEIPT_BYTES {
        return Err(format!(
            "{name} is empty or exceeds the recovery receipt limit"
        ));
    }
    serde_json::from_slice(bytes).map_err(|error| format!("Invalid {name}: {error}"))
}

fn path_text(path: &Path) -> Result<String, String> {
    let value = path
        .to_str()
        .ok_or("Recovery paths must be valid Unicode")?;
    #[cfg(windows)]
    let value = if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        value.strip_prefix(r"\\?\").unwrap_or(value).to_owned()
    };
    #[cfg(not(windows))]
    let value = value.to_owned();
    Ok(value)
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (path_text(left), path_text(right)) {
        #[cfg(windows)]
        (Ok(left), Ok(right)) => left
            .replace('/', "\\")
            .eq_ignore_ascii_case(&right.replace('/', "\\")),
        #[cfg(not(windows))]
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn validate_script_paths(expected: &Expected<'_>) -> Result<(), String> {
    if super::options::plugin(expected.options).is_none() {
        return if expected.script_text.is_empty() {
            Ok(())
        } else {
            Err("An FFmpeg source must not contain an executable source script".into())
        };
    }
    if expected.script_text.is_empty() || expected.script_text.len() > MAX_RECEIPT_BYTES {
        return Err("The recovery source script is empty or oversized".into());
    }
    // av1an writes the input spelling verbatim, but dunce-normalizes its cache.
    let source = expected
        .source
        .to_str()
        .ok_or("Recovery paths must be valid Unicode")?;
    let extension = match expected.options.chunk_method {
        Av1anChunkMethod::Ffms2 => "ffindex",
        Av1anChunkMethod::Bestsource => "bsindex",
        _ => "lwi",
    };
    let cache = path_text(
        &expected
            .chunks_directory
            .join(format!("split/cache.{extension}")),
    )?;
    if [source, cache.as_str()]
        .iter()
        .any(|path| path.contains(['"', '\n', '\r']))
    {
        return Err(
            "The recovery source or cache path cannot be represented safely in the source script"
                .into(),
        );
    }
    for (prefix, wanted) in [
        ("source =", format!("source = r\"{source}\"")),
        (
            "chunk_method =",
            format!(
                "chunk_method = \"{}\"",
                super::options::chunk_method(expected.options)
            ),
        ),
        ("cache_mode =", "cache_mode = \"temp\"".into()),
        ("cache_file =", format!("cache_file = r\"{cache}\"")),
    ] {
        let values: Vec<_> = expected
            .script_text
            .lines()
            .filter(|line| line.starts_with(prefix))
            .collect();
        if values != [wanted.as_str()] {
            return Err(
                "The recovery source script does not match the source, cache, or chunk method"
                    .into(),
            );
        }
    }
    Ok(())
}

fn validate_script_template(script: &str) -> Result<(), String> {
    let normalized = script
        .replace("\r\n", "\n")
        .split_inclusive('\n')
        .map(|line| {
            if line.starts_with("source =") {
                "source = <SOURCE>\n"
            } else if line.starts_with("chunk_method =") {
                "chunk_method = \"lsmash\"\n"
            } else if line.starts_with("cache_file =") {
                "cache_file = <CACHE>\n"
            } else {
                line
            }
        })
        .collect::<String>();
    if format!("{:x}", Sha256::digest(normalized.as_bytes())) != SOURCE_SCRIPT_SHA256 {
        return Err(
            "The recovery source script does not match the supported av1an template".into(),
        );
    }
    Ok(())
}

fn scene_coverage(scenes: &[Scene], total: u64, max_length: Option<u64>) -> bool {
    let mut next = 0;
    if scenes.is_empty() || scenes.len() > MAX_CHUNKS {
        return false;
    }
    for scene in scenes {
        if scene.start_frame != next
            || scene.end_frame <= next
            || scene.end_frame > total
            || scene.zone_overrides.is_some()
            || max_length.is_some_and(|limit| scene.end_frame - next > limit)
        {
            return false;
        }
        next = scene.end_frame;
    }
    next == total
}

pub(super) fn validate(
    chunks_bytes: &[u8],
    scenes_bytes: &[u8],
    done_bytes: &[u8],
    expected: &Expected<'_>,
) -> Result<Receipts, String> {
    validate_script_paths(expected)?;
    if super::options::plugin(expected.options).is_some() {
        validate_script_template(expected.script_text)?;
    }
    validate_receipt_shapes(chunks_bytes, scenes_bytes, done_bytes, expected)
}

fn validate_receipt_shapes(
    chunks_bytes: &[u8],
    scenes_bytes: &[u8],
    done_bytes: &[u8],
    expected: &Expected<'_>,
) -> Result<Receipts, String> {
    if expected.total_frames == 0
        || expected.fps_num == 0
        || expected.fps_den == 0
        || expected.source_fps_num == 0
        || expected.source_fps_den == 0
        || !expected.source.is_absolute()
        || !expected.chunks_directory.is_absolute()
    {
        return Err("Invalid expected recovery plan".into());
    }
    validate_script_paths(expected)?;
    let mut chunks: Vec<Chunk> = parse(chunks_bytes, "chunks.json")?;
    let scenes: Scenes = parse(scenes_bytes, "scenes.json")?;
    let done: Done = parse(done_bytes, "done.json")?;
    if chunks.is_empty()
        || chunks.len() > MAX_CHUNKS
        || scenes.frames != expected.total_frames
        || done.frames != expected.total_frames
        || !scene_coverage(&scenes.scenes, expected.total_frames, None)
        || !scene_coverage(
            &scenes.split_scenes,
            expected.total_frames,
            (expected.options.maximum_chunk_frames > 0)
                .then_some(u64::from(expected.options.maximum_chunk_frames)),
        )
        || chunks.len() != scenes.split_scenes.len()
    {
        return Err("Recovery scenes and chunks do not cover the validated source exactly".into());
    }
    let script_path = expected.chunks_directory.join("split/loadscript.vpy");
    let source_fps = f64::from(expected.source_fps_num) / f64::from(expected.source_fps_den);
    chunks.sort_by_key(|chunk| chunk.index);
    let mut segments = Vec::<PathBuf>::new();
    let mut segment_offset = 0;
    for (index, (chunk, scene)) in chunks.iter().zip(&scenes.split_scenes).enumerate() {
        let quality = &chunk.target_quality;
        if chunk.index != index
            || (expected.options.chunk_method != Av1anChunkMethod::Hybrid
                && (chunk.start_frame != scene.start_frame || chunk.end_frame != scene.end_frame))
            || chunk.end_frame.checked_sub(chunk.start_frame)
                != Some(scene.end_frame - scene.start_frame)
            || !same_path(&chunk.temp, expected.chunks_directory)
            || chunk.proxy.is_some()
            || chunk.proxy_cmd.is_some()
            || chunk.output_ext != "ivf"
            || chunk.encoder != "svt_av1"
            || chunk.passes != 1
            || chunk.video_params != expected.video_params
            || chunk.noise_size != (None, None)
            || chunk.ignore_frame_mismatch
            || chunk.per_shot_target_quality_cq.is_some_and(|quality| {
                !matches!(
                    expected.options.chunk_method,
                    Av1anChunkMethod::Select | Av1anChunkMethod::Hybrid
                ) || expected.options.target_quality.is_none_or(|target| {
                    !quality.is_finite()
                        || quality < f64::from(target.minimum_crf)
                        || quality > f64::from(target.maximum_crf)
                })
            })
            || !chunk.frame_rate.is_finite()
            || (chunk.frame_rate - source_fps).abs() > source_fps.abs() * f64::EPSILON * 2.0
            || !quality.matches_plan(expected)
        {
            return Err(format!(
                "Recovery chunk {index} does not match the immutable encoding plan"
            ));
        }
        let argv = &chunk.source_cmd;
        let valid_source = match &chunk.input {
            Input::VapourSynth(input) => {
                super::options::plugin(expected.options).is_some()
                    && same_path(&input.path, &script_path)
                    && input.script_text == expected.script_text
                    && input.vspipe_args.is_empty()
                    && !input.is_proxy
                    && argv.len() == 9
                    && argv[0] == "vspipe"
                    && same_path(Path::new(&argv[1]), &script_path)
                    && argv[2] == "-c"
                    && argv[3] == "y4m"
                    && argv[4] == "-"
                    && argv[5] == "-s"
                    && argv[6] == chunk.start_frame.to_string().as_str()
                    && argv[7] == "-e"
                    && argv[8] == (chunk.end_frame - 1).to_string().as_str()
            }
            Input::Video(input) => {
                let input_valid = if expected.options.chunk_method == Av1anChunkMethod::Hybrid {
                    if segments
                        .last()
                        .is_none_or(|path| !same_path(path, &input.path))
                    {
                        if chunk.start_frame != 0 {
                            return Err("Hybrid segment does not start at its first frame".into());
                        }
                        let name = format!("{:05}.mkv", segments.len());
                        if !same_path(
                            &input.path,
                            &expected.chunks_directory.join("split").join(name),
                        ) && !(segments.is_empty()
                            && same_path(
                                &input.path,
                                &expected.chunks_directory.join("split/0.mkv"),
                            ))
                        {
                            return Err(
                                "Hybrid source segment is outside its exact owned path".into()
                            );
                        }
                        segments.push(input.path.clone());
                        segment_offset = scene.start_frame;
                    }
                    chunk.start_frame.checked_add(segment_offset) == Some(scene.start_frame)
                        && chunk.end_frame.checked_add(segment_offset) == Some(scene.end_frame)
                } else {
                    expected.options.chunk_method == Av1anChunkMethod::Select
                        && same_path(&input.path, expected.source)
                };
                let wanted: Vec<OsString> =
                    ["ffmpeg", "-y", "-hide_banner", "-loglevel", "error", "-i"]
                        .into_iter()
                        .map(OsString::from)
                        .chain(std::iter::once(input.path.as_os_str().to_owned()))
                        .chain(
                            [
                                "-vf".to_owned(),
                                format!(
                                    r"select=between(n\,{}\,{}),setpts=PTS-STARTPTS",
                                    chunk.start_frame,
                                    chunk.end_frame - 1
                                ),
                                "-pix_fmt".into(),
                                "yuv420p10le".into(),
                                "-strict".into(),
                                "-1".into(),
                                "-fps_mode".into(),
                                "passthrough".into(),
                                "-f".into(),
                                "yuv4mpegpipe".into(),
                                "-".into(),
                            ]
                            .into_iter()
                            .map(OsString::from),
                        )
                        .collect();
                input_valid
                    && same_path(&input.temp, expected.chunks_directory)
                    && input.chunk_method == "Select"
                    && input.cache_mode == "TEMP"
                    && !input.is_proxy
                    && *argv == wanted
            }
        };
        if !valid_source {
            return Err(format!(
                "Recovery chunk {index} has an unexpected source command or reader"
            ));
        }
    }
    if segments.len() > 1 && segments[0].file_name().is_some_and(|name| name == "0.mkv") {
        return Err("Hybrid segmented paths mix incompatible naming modes".into());
    }
    let mut completed = Vec::new();
    let mut completed_frames = 0u64;
    for (name, record) in done.done.0 {
        let index: usize = name.parse().map_err(|_| "Invalid completed chunk name")?;
        let chunk = chunks
            .get(index)
            .ok_or("Completed chunk is not in the encoding queue")?;
        if name != format!("{index:05}")
            || record.frames != chunk.end_frame - chunk.start_frame
            || record.size_bytes <= 32
        {
            return Err("Completed chunk counts or sizes do not match the encoding queue".into());
        }
        completed_frames = completed_frames
            .checked_add(record.frames)
            .ok_or("Completed frame count overflow")?;
        completed.push(CompletedChunk {
            name,
            index,
            frames: record.frames,
            size_bytes: record.size_bytes,
        });
    }
    Ok(Receipts {
        completed,
        completed_frames,
        total_frames: expected.total_frames,
        queued_chunks: chunks.len(),
        segments,
        script_path: super::options::plugin(expected.options)
            .is_some()
            .then_some(script_path),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    // Generated by av1an 0.5.2-unstable (7df934d), GPL-3.0.
    // https://github.com/rust-av/Av1an/tree/7df934d
    const SCRIPT_TEMPLATE: &str = r#"import os
import vapoursynth as vs

core = vs.core

source = <SOURCE>
chunk_method = "lsmash"
perform_scene_detection = globals().get("AV1AN_PERFORM_SCENE_DETECTION", None)
cache_mode = "temp"
cache_file = <CACHE>
pix_fmt = os.environ.get("AV1AN_PIXEL_FORMAT", None)

# Import video
match (chunk_method):  # type: ignore
    case "lsmash":
        if cache_mode == "temp":
            video = core.lsmas.LWLibavSource(source, cachefile=cache_file)
        else:
            video = core.lsmas.LWLibavSource(source)
    case "ffms2":
        if cache_mode == "temp":
            video = core.ffms2.Source(source, cachefile=cache_file)
        else:
            video = core.ffms2.Source(source)
    case "dgdecnv":
        video = core.dgdecodenv.DGSource(source)
    case "bestsource":
        if cache_mode == "temp":
            try:
                video = core.bs.VideoSource(source, cachepath=cache_file, cachemode=4)
            except Exception:
                video = core.bs.VideoSource(source, cachepath=cache_file)
        else:
            # bestsource has the default behavior to store its index files in a user-specific directory
            # but for consistency, this setting makes it store the index file next to the video
            # as all the other source filters do
            video = core.bs.VideoSource(source, cachepath="/")

if perform_scene_detection is None:
    # Limit decoder resources when encoding since we will have multiple workers running
    core.num_threads = 1
    core.max_cache_size = 1024

if pix_fmt is not None:
    video = video.resize.Bicubic(format=vs.PresetVideoFormat[pix_fmt])

# Output video
video.set_output()
"#;

    struct Fixture {
        source: PathBuf,
        options: Av1anOptions,
        directory: PathBuf,
        script: String,
        params: Vec<String>,
        queue: Value,
        scenes: Value,
        done: Value,
    }
    impl Fixture {
        fn new() -> Self {
            let root = PathBuf::from(if cfg!(windows) {
                r"C:\jesses-receipts"
            } else {
                "/jesses-receipts"
            });
            let source = root.join("source.mkv");
            let directory = root.join("chunks");
            let script_path = directory.join("split/loadscript.vpy");
            let script = SCRIPT_TEMPLATE
                .replace("<SOURCE>", &format!("r\"{}\"", source.display()))
                .replace(
                    "<CACHE>",
                    &format!("r\"{}\"", directory.join("split/cache.lwi").display()),
                );
            let params = vec!["--crf".into(), "30".into()];
            let queue = json!((0..2).map(|index| {
                let start = index * 24;
                let end = start + 24;
                let args: Vec<OsString> = vec!["vspipe".into(),script_path.clone().into_os_string(),"-c".into(),"y4m".into(),"-".into(),"-s".into(),start.to_string().into(),"-e".into(),(end-1).to_string().into()];
                json!({"temp":directory,"index":index,"input":{"VapourSynth":{"path":script_path,"vspipe_args":[],"script_text":script,"is_proxy":false}},"proxy":null,"source_cmd":args,"proxy_cmd":null,"output_ext":"ivf","start_frame":start,"end_frame":end,"frame_rate":24.0,"passes":1,"video_params":params,"encoder":"svt_av1","noise_size":[null,null],"target_quality":{"vmaf_res":"1920x1080","probe_res":null,"vmaf_scaler":"bicubic","vmaf_filter":null,"vmaf_threads":1,"model":null,"probing_rate":1,"probes":4,"target":null,"metric":"VMAF","min_q":15,"max_q":50,"interp_method":null,"encoder":"svt_av1","pix_format":"YUV420P10LE","temp":directory,"workers":1,"video_params":null,"params_copied":false,"vspipe_args":[],"probing_vmaf_features":["Default"],"probing_statistic":{"name":"Automatic","value":null}},"per_shot_target_quality_cq":null,"ignore_frame_mismatch":false})
            }).collect::<Vec<_>>());
            let scenes = json!({"frames":48,"scenes":[{"start_frame":0,"end_frame":48,"zone_overrides":null}],"split_scenes":[{"start_frame":0,"end_frame":24,"zone_overrides":null},{"start_frame":24,"end_frame":48,"zone_overrides":null}]});
            let done = json!({"frames":48,"done":{"00000":{"frames":24,"size_bytes":4096}},"audio_done":true});
            Self {
                source,
                options: Av1anOptions::default(),
                directory,
                script,
                params,
                queue,
                scenes,
                done,
            }
        }
        fn expected(&self) -> Expected<'_> {
            Expected {
                source: &self.source,
                chunks_directory: &self.directory,
                video_params: &self.params,
                total_frames: 48,
                fps_num: 24,
                fps_den: 1,
                source_fps_num: 24,
                source_fps_den: 1,
                script_text: &self.script,
                options: self.options,
                source_filter: None,
            }
        }
        fn validate(&self) -> Result<Receipts, String> {
            validate(
                &serde_json::to_vec(&self.queue).unwrap(),
                &serde_json::to_vec(&self.scenes).unwrap(),
                &serde_json::to_vec(&self.done).unwrap(),
                &self.expected(),
            )
        }
    }

    #[test]
    fn configured_readers_and_quality_receipts_remain_immutable() {
        use media_core::Av1anTargetQuality;
        for (method, name, extension) in [
            (Av1anChunkMethod::Ffms2, "ffms2", "ffindex"),
            (Av1anChunkMethod::Bestsource, "bestsource", "bsindex"),
        ] {
            let mut fixture = Fixture::new();
            fixture.options.chunk_method = method;
            fixture.script = fixture
                .script
                .replace(
                    "chunk_method = \"lsmash\"",
                    &format!("chunk_method = \"{name}\""),
                )
                .replace("cache.lwi", &format!("cache.{extension}"));
            for chunk in fixture.queue.as_array_mut().unwrap() {
                chunk["input"]["VapourSynth"]["script_text"] = json!(fixture.script);
            }
            fixture.validate().unwrap();
            fixture.options.chunk_method = Av1anChunkMethod::Lsmash;
            assert!(fixture.validate().is_err());
        }
        let mut fixture = Fixture::new();
        fixture.options.maximum_chunk_frames = 24;
        fixture.options.target_quality = Some(Av1anTargetQuality {
            metric: Default::default(),
            minimum_score_tenths: 930,
            maximum_score_tenths: 960,
            minimum_crf: 18,
            maximum_crf: 44,
            probes: 3,
            probing_rate: 2,
            probe_width: 640,
            probe_height: 360,
        });
        for chunk in fixture.queue.as_array_mut().unwrap() {
            let quality = &mut chunk["target_quality"];
            for (key, value) in [
                ("vmaf_res", json!("640x360")),
                ("probe_res", json!([640, 360])),
                ("vmaf_threads", json!(2)),
                ("probing_rate", json!(2)),
                ("probes", json!(3)),
                ("target", json!([93.0, 96.0])),
                ("min_q", json!(18)),
                ("max_q", json!(44)),
                ("video_params", json!(fixture.params)),
                ("probing_statistic", json!({"name":"Mean", "value":null})),
            ] {
                quality[key] = value;
            }
        }
        fixture.validate().unwrap();
        let original = fixture.queue.clone();
        for (key, value) in [
            ("probe_res", json!([1280, 720])),
            ("video_params", json!(["--crf", "5"])),
            ("target", json!([91.0, 96.0])),
            ("params_copied", json!(true)),
            ("vmaf_filter", json!("crop=12:12")),
            ("vmaf_threads", json!(9)),
        ] {
            fixture.queue[0]["target_quality"][key] = value;
            assert!(fixture.validate().is_err(), "modified {key}");
            fixture.queue = original.clone();
        }
        for metric in [
            media_core::Av1anTargetMetric::Ssimulacra2,
            media_core::Av1anTargetMetric::Butteraugli,
            media_core::Av1anTargetMetric::Xpsnr,
        ] {
            fixture.options.target_quality.as_mut().unwrap().metric = metric;
            assert!(
                fixture.validate().is_err(),
                "VMAF receipt cannot satisfy a different metric"
            );
            for chunk in fixture.queue.as_array_mut().unwrap() {
                chunk["target_quality"]["metric"] = json!(super::super::metrics::receipt(metric));
            }
            fixture.validate().unwrap();
            fixture.queue = original.clone();
        }
        fixture.options.target_quality.as_mut().unwrap().metric =
            media_core::Av1anTargetMetric::Vmaf;
        fixture.options.maximum_chunk_frames = 12;
        assert!(
            fixture.validate().is_err(),
            "configured maximum must constrain receipt coverage"
        );
    }

    #[test]
    fn validates_exact_frame_coverage_and_completed_chunk_identity() {
        let fixture = Fixture::new();
        let receipts = fixture.validate().unwrap();
        assert_eq!(receipts.completed_frames, 24);
        assert_eq!(receipts.total_frames, 48);
        assert_eq!(receipts.queued_chunks, 2);
        assert_eq!(
            receipts.script_path,
            Some(fixture.directory.join("split/loadscript.vpy"))
        );
        assert_eq!(
            receipts.completed,
            vec![CompletedChunk {
                name: "00000".into(),
                index: 0,
                frames: 24,
                size_bytes: 4096
            }]
        );
    }

    #[test]
    fn validates_chunks_by_index_when_engine_orders_queue_by_work_size() {
        let mut fixture = Fixture::new();
        fixture.queue.as_array_mut().unwrap().reverse();
        let receipts = fixture.validate().unwrap();
        assert_eq!(receipts.completed[0].index, 0);
        assert_eq!(receipts.completed[0].name, "00000");
        assert_eq!(receipts.completed_frames, 24);
        fixture.queue[0]["index"] = json!(0);
        assert!(
            fixture.validate().is_err(),
            "Duplicate indices must still be rejected"
        );
    }

    #[test]
    fn nominal_queue_rate_does_not_replace_validated_decimal_encoding_cadence() {
        let mut fixture = Fixture::new();
        fixture.params = ["--fps-num", "2997", "--fps-denom", "125"]
            .map(String::from)
            .to_vec();
        for chunk in fixture.queue.as_array_mut().unwrap() {
            chunk["frame_rate"] = json!(24000.0 / 1001.0);
            chunk["video_params"] = json!(fixture.params);
        }
        fixture.queue.as_array_mut().unwrap().reverse();
        let check = |fixture: &Fixture, source_rate: (u32, u32)| {
            let mut expected = fixture.expected();
            expected.fps_num = 2997;
            expected.fps_den = 125;
            expected.source_fps_num = source_rate.0;
            expected.source_fps_den = source_rate.1;
            validate(
                &serde_json::to_vec(&fixture.queue).unwrap(),
                &serde_json::to_vec(&fixture.scenes).unwrap(),
                &serde_json::to_vec(&fixture.done).unwrap(),
                &expected,
            )
        };
        check(&fixture, (24000, 1001)).unwrap();
        assert!(
            check(&fixture, (2997, 125)).is_err(),
            "Queue rate must match independently supplied source metadata"
        );
        for incorrect in [23.976, 24.0, 23.976024] {
            fixture.queue[0]["frame_rate"] = json!(incorrect);
            assert!(
                check(&fixture, (24000, 1001)).is_err(),
                "Incorrect nominal rate {incorrect}"
            );
        }
        fixture.queue[0]["frame_rate"] = json!(24000.0 / 1001.0);
        fixture.queue[0]["video_params"][1] = json!("24000");
        fixture.queue[0]["video_params"][3] = json!("1001");
        assert!(
            check(&fixture, (24000, 1001)).is_err(),
            "Encoder rate must remain the exact validated decimal cadence"
        );
    }

    #[test]
    fn rejects_changed_executable_paths_arguments_settings_and_coverage() {
        for (pointer, value) in [
            (
                "/0/source_cmd/0",
                serde_json::to_value(OsString::from("untrusted-tool")).unwrap(),
            ),
            (
                "/0/source_cmd/1",
                serde_json::to_value(OsString::from("elsewhere.vpy")).unwrap(),
            ),
            (
                "/0/source_cmd/8",
                serde_json::to_value(OsString::from("99")).unwrap(),
            ),
            ("/0/video_params/1", json!("12")),
            ("/0/start_frame", json!(1)),
            ("/1/index", json!(0)),
            ("/0/input/VapourSynth/script_text", json!("untrusted code")),
            ("/0/input/VapourSynth/path", json!("elsewhere.vpy")),
            ("/0/target_quality/target", json!(90)),
            ("/0/ignore_frame_mismatch", json!(true)),
        ] {
            let mut fixture = Fixture::new();
            *fixture.queue.pointer_mut(pointer).unwrap() = value;
            assert!(fixture.validate().is_err(), "{pointer}");
        }
        let mut fixture = Fixture::new();
        fixture.scenes["split_scenes"][1]["start_frame"] = json!(25);
        assert!(fixture.validate().is_err());
        let mut fixture = Fixture::new();
        fixture.done["done"]["00000"]["frames"] = json!(25);
        assert!(fixture.validate().is_err());
        let mut fixture = Fixture::new();
        fixture.script = fixture.script.replace("source =", "changed_source =");
        assert!(fixture.validate().is_err());
    }

    #[test]
    fn rejects_duplicate_done_keys_truncation_and_oversized_receipts() {
        let fixture = Fixture::new();
        let queue = serde_json::to_vec(&fixture.queue).unwrap();
        let scenes = serde_json::to_vec(&fixture.scenes).unwrap();
        let done=br#"{"frames":48,"done":{"00000":{"frames":24,"size_bytes":4096},"00000":{"frames":24,"size_bytes":4096}},"audio_done":true}"#;
        assert!(validate(&queue, &scenes, done, &fixture.expected()).is_err());
        assert!(validate(&queue, &scenes, b"{", &fixture.expected()).is_err());
        assert!(
            validate(
                &vec![b' '; MAX_RECEIPT_BYTES + 1],
                &scenes,
                done,
                &fixture.expected()
            )
            .is_err()
        );
    }

    #[test]
    fn rejects_modified_python_even_when_queue_and_script_agree() {
        for script in [
            format!("{}\nraise RuntimeError('changed')\n", Fixture::new().script),
            Fixture::new()
                .script
                .replace("video.set_output()", "raise RuntimeError('changed')"),
        ] {
            let mut fixture = Fixture::new();
            fixture.script = script;
            for chunk in fixture.queue.as_array_mut().unwrap() {
                chunk["input"]["VapourSynth"]["script_text"] = json!(fixture.script);
            }
            assert!(
                fixture
                    .validate()
                    .unwrap_err()
                    .contains("supported av1an template")
            );
        }
        let mut fixture = Fixture::new();
        fixture.queue[0]["target_quality"]["unknown_execution_option"] = json!("changed");
        assert!(fixture.validate().is_err());
    }

    #[test]
    fn accepts_only_expected_source_spelling_and_crlf_template() {
        let mut fixture = Fixture::new();
        #[cfg(windows)]
        {
            let source = PathBuf::from(format!(r"\\?\{}", fixture.source.display()));
            fixture.script = fixture.script.replace(
                &format!("source = r\"{}\"", fixture.source.display()),
                &format!("source = r\"{}\"", source.display()),
            );
            fixture.source = source;
        }
        fixture.script = fixture.script.replace('\n', "\r\n");
        for chunk in fixture.queue.as_array_mut().unwrap() {
            chunk["input"]["VapourSynth"]["script_text"] = json!(fixture.script);
        }
        fixture.validate().unwrap();
    }
}
