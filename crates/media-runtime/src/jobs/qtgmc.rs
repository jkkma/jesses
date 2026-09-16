//! Managed QTGMC scripts. User paths are data literals and the selected
//! frameserver is exercised before any output is created.
use super::{NEXT_ID, discover, encode_plan::Plan, files, process_error, rate_control::Stats};
use crate::supervisor::{self, ChildEnvironment, CommandSpec};
use media_core::{AppError, DeinterlaceMode, FieldOrder, QtgmcPreset, QtgmcSettings};
use std::{
    ffi::OsString,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::watch;

struct Runtime {
    vspipe: PathBuf,
    environment: Option<ChildEnvironment>,
}

fn optional_av1an(result: Result<PathBuf, AppError>) -> Result<Option<PathBuf>, AppError> {
    match result {
        Ok(path) => Ok(Some(path)),
        Err(error) if error.code == "TOOL_MISSING" => Ok(None),
        Err(error) => Err(error),
    }
}

async fn runtime(cancel: &watch::Receiver<bool>) -> Result<Runtime, AppError> {
    let av1an = optional_av1an(discover("av1an", cancel).await)?;
    if let Some(av1an) = av1an {
        let bundled = crate::bundled_tools::av1an_runtime(&av1an)
            .map_err(|detail| files::error("QTGMC_DEPENDENCY_MISSING", detail, &av1an))?;
        let portable = av1an.parent().expect("absolute av1an path").join("vsynth");
        let directory = bundled.or_else(|| portable.is_dir().then_some(portable));
        if let Some(directory) = directory {
            let name = if cfg!(windows) {
                "VSPipe.exe"
            } else {
                "vspipe"
            };
            let vspipe = directory.join(name).canonicalize().map_err(|error| {
                files::error(
                    "QTGMC_DEPENDENCY_MISSING",
                    format!("The selected frameserver runtime has no VSPipe: {error}"),
                    &directory,
                )
            })?;
            let mut path = directory.as_os_str().to_owned();
            if let Some(inherited) = std::env::var_os("PATH") {
                path.push(if cfg!(windows) { ";" } else { ":" });
                path.push(inherited);
            }
            return Ok(Runtime {
                vspipe,
                environment: Some(crate::bundled_tools::frameserver_environment(
                    &directory, &path,
                )),
            });
        }
    }
    let vspipe = crate::discovery::find_executable(&["VSPipe", "vspipe"])
        .await
        .map_err(|detail| AppError::new("QTGMC_DEPENDENCY_MISSING", detail, None))?
        .ok_or_else(|| {
            AppError::new(
                "QTGMC_DEPENDENCY_MISSING",
                "QTGMC requires VSPipe plus havsfunc and its VapourSynth plugins.",
                None,
            )
        })?;
    Ok(Runtime {
        vspipe,
        environment: None,
    })
}

fn preset(preset: QtgmcPreset) -> &'static str {
    match preset {
        QtgmcPreset::Faster => "Faster",
        QtgmcPreset::Fast => "Fast",
        QtgmcPreset::Medium => "Medium",
        QtgmcPreset::Slow => "Slow",
        QtgmcPreset::Slower => "Slower",
    }
}

fn literal(path: &Path) -> Result<String, AppError> {
    let value = path.to_str().ok_or_else(|| {
        files::error(
            "QTGMC_INPUT_UNSUPPORTED",
            "QTGMC source and cache paths must be valid Unicode.",
            path,
        )
    })?;
    serde_json::to_string(value)
        .map_err(|error| files::error("QTGMC_PREPARE_FAILED", error.to_string(), path))
}

pub(super) fn source_script(
    source: &Path,
    cache: &Path,
    stream_index: u32,
    trim: Option<(u32, u32)>,
    settings: QtgmcSettings,
) -> Result<String, AppError> {
    let source = literal(source)?;
    let cache = literal(cache)?;
    let selection = trim.map_or_else(String::new, |(start, end)| {
        format!("clip = clip[{start}:{end}]\n")
    });
    Ok(format!(
        "import vapoursynth as vs\nimport havsfunc\ncore = vs.core\nclip = core.lsmas.LWLibavSource(source={source}, stream_index={stream_index}, cachefile={cache})\nclip = core.std.SetFrameProps(clip, _FieldBased={field})\n{selection}clip = havsfunc.QTGMC(clip, Preset={preset:?}, TFF={tff}, FPSDivisor={divisor})\nclip = core.std.SetFrameProps(clip, _FieldBased=0)\nclip.set_output()\n",
        field = if settings.field_order == FieldOrder::TopFirst {
            2
        } else {
            1
        },
        preset = preset(settings.preset),
        tff = if settings.field_order == FieldOrder::TopFirst {
            "True"
        } else {
            "False"
        },
        divisor = if settings.mode == DeinterlaceMode::Frame {
            2
        } else {
            1
        },
    ))
}

pub(super) struct Script {
    pub path: PathBuf,
    file: Option<std::fs::File>,
}

impl Script {
    pub fn create(work: &Path, content: &str) -> Result<Self, AppError> {
        let path = work.join(format!(
            "qtgmc-{}-{}.vpy",
            std::process::id(),
            NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            options.share_mode(1);
        }
        let mut file = options
            .open(&path)
            .map_err(|error| files::error("QTGMC_PREPARE_FAILED", error.to_string(), &path))?;
        file.write_all(content.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| files::error("QTGMC_PREPARE_FAILED", error.to_string(), &path))?;
        Ok(Self {
            path,
            file: Some(file),
        })
    }
}

impl Drop for Script {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.path);
    }
}

pub(super) struct Prepared {
    pub video: files::Temporary,
    pub producer: CommandSpec,
    pub consumer: CommandSpec,
    environment: Option<ChildEnvironment>,
    _script: Script,
    cache: PathBuf,
    workspace: Stats,
}

impl Prepared {
    #[allow(clippy::too_many_arguments)]
    pub async fn build(
        source: &Path,
        output: &Path,
        id: &str,
        plan: &Plan,
        trim: Option<(u32, u32)>,
        ffmpeg: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<Self, AppError> {
        let settings = plan.qtgmc_settings().expect("QTGMC plan");
        let runtime = runtime(cancel).await?;
        let workspace = Stats::create(output, &format!("{id}-qtgmc"))?;
        check_tools(
            &runtime.vspipe,
            runtime.environment.as_ref(),
            &workspace.path,
            settings,
            cancel,
        )
        .await?;
        let cache = workspace.path.join("jesses.stats.qtgmc-cache.lwi");
        let script = Script::create(
            &workspace.path,
            &source_script(source, &cache, plan.video_index, trim, settings)?,
        )?;
        let video = files::Temporary::create(output, &format!("{id}-qtgmc-video"))?;
        let producer = CommandSpec {
            executable: runtime.vspipe,
            args: vec![
                script.path.as_os_str().to_owned(),
                "-".into(),
                "-c".into(),
                "y4m".into(),
                "--progress".into(),
            ],
            cwd: Some(workspace.path.clone()),
        };
        let mut consumer_args: Vec<OsString> = [
            "-hide_banner",
            "-nostdin",
            "-v",
            "warning",
            "-xerror",
            "-f",
            "yuv4mpegpipe",
            "-i",
            "pipe:0",
            "-map",
            "0:v:0",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        if let Some(filter) = plan.post_qtgmc_filter_with_text(None) {
            consumer_args.extend(["-vf".into(), filter.into()]);
        }
        consumer_args.extend([
            "-r".into(),
            format!("{}/{}", plan.fps_num, plan.fps_den).into(),
            "-an".into(),
            "-sn".into(),
            "-dn".into(),
            "-fps_mode".into(),
            "cfr".into(),
            "-pix_fmt".into(),
            plan.output_pixel_format.into(),
            "-color_primaries".into(),
            plan.primaries.to_string().into(),
            "-color_trc".into(),
            plan.transfer.to_string().into(),
            "-colorspace".into(),
            plan.matrix.to_string().into(),
            "-color_range".into(),
            if plan.full_range { "pc" } else { "tv" }.into(),
            "-chroma_sample_location".into(),
            plan.chroma.into(),
            "-c:v".into(),
            "ffv1".into(),
            "-level".into(),
            "3".into(),
            "-g".into(),
            "1".into(),
            "-slicecrc".into(),
            "1".into(),
            "-f".into(),
            "matroska".into(),
            "pipe:1".into(),
        ]);
        Ok(Self {
            video,
            producer,
            consumer: CommandSpec {
                executable: ffmpeg.to_owned(),
                args: consumer_args,
                cwd: None,
            },
            environment: runtime.environment,
            _script: script,
            cache,
            workspace,
        })
    }

    pub async fn run(
        &self,
        cancel: watch::Receiver<bool>,
        events: tokio::sync::mpsc::Sender<crate::supervisor::ProcessEvent>,
        log: &Path,
    ) -> Result<(), AppError> {
        let output = self.video.clone_file()?;
        let result = if let Some(environment) = &self.environment {
            supervisor::run_pipeline_to_file_with_producer_environment(
                &self.producer,
                &self.consumer,
                cancel,
                events,
                log,
                Duration::from_secs(24 * 60 * 60),
                output,
                environment,
            )
            .await
        } else {
            supervisor::run_pipeline_to_file(
                &self.producer,
                &self.consumer,
                cancel,
                events,
                log,
                Duration::from_secs(24 * 60 * 60),
                output,
            )
            .await
        }
        .map_err(|error| process_error(error, &self.video.path))?;
        if !result.producer_status.success() || !result.consumer_status.success() {
            return Err(files::error(
                "QTGMC_PREPARE_FAILED",
                "VSPipe or FFmpeg failed while producing the verified QTGMC intermediate.",
                &self.video.path,
            ));
        }
        self.workspace.check()?;
        let cache = std::fs::symlink_metadata(&self.cache).map_err(|error| {
            files::error("QTGMC_PREPARE_FAILED", error.to_string(), &self.cache)
        })?;
        if !cache.is_file() || cache.len() == 0 {
            return Err(files::error(
                "QTGMC_PREPARE_FAILED",
                "The source reader produced no owned index cache.",
                &self.cache,
            ));
        }
        self.video.flush_nonempty_async().await
    }

    pub async fn validate(
        &self,
        ffprobe: &Path,
        plan: &Plan,
        expected_frames: usize,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        let document = super::probe(ffprobe, &self.video.path, cancel, Some(&[0])).await?;
        let stream = document.streams.first().ok_or_else(|| {
            files::error(
                "QTGMC_PREPARE_FAILED",
                "The QTGMC intermediate has no video stream.",
                &self.video.path,
            )
        })?;
        let primaries = match plan.primaries {
            1 => "bt709",
            5 => "bt470bg",
            6 => "smpte170m",
            9 => "bt2020",
            _ => "unknown",
        };
        let transfer = match plan.transfer {
            1 => "bt709",
            5 => "bt470bg",
            6 => "smpte170m",
            16 => "smpte2084",
            18 => "arib-std-b67",
            _ => "unknown",
        };
        let matrix = match plan.matrix {
            1 => "bt709",
            5 => "bt470bg",
            6 => "smpte170m",
            9 => "bt2020nc",
            _ => "unknown",
        };
        let rate_ok = stream
            .avg_frame_rate
            .as_deref()
            .and_then(|value| value.split_once('/'))
            .and_then(|(num, den)| Some((num.parse::<u64>().ok()?, den.parse::<u64>().ok()?)))
            .is_some_and(|(num, den)| {
                num * u64::from(plan.fps_den) == den * u64::from(plan.fps_num)
            });
        if document.streams.len() != 1
            || stream.codec_name.as_deref() != Some("ffv1")
            || stream.width != Some(plan.width)
            || stream.height != Some(plan.height)
            || !plan.matches_output_format(stream.pix_fmt.as_deref())
            || stream.sample_aspect_ratio.as_deref() != Some(plan.output_sar())
            || !rate_ok
            || stream.color_primaries.as_deref() != Some(primaries)
            || stream.color_transfer.as_deref() != Some(transfer)
            || stream.color_space.as_deref() != Some(matrix)
            || stream.color_range.as_deref() != Some(if plan.full_range { "pc" } else { "tv" })
        {
            return Err(files::error(
                "QTGMC_PREPARE_FAILED",
                format!(
                    "The lossless QTGMC intermediate differs from the validated processing plan (codec={:?}, streams={}, geometry={:?}x{:?}, pixel={:?}, SAR={:?}, rate={:?}, color={:?}/{:?}/{:?}/{:?}; expected FFV1, one stream, {}x{}, {}, SAR {}, {}/{} fps, {}/{}/{}/{}).",
                    stream.codec_name,
                    document.streams.len(),
                    stream.width,
                    stream.height,
                    stream.pix_fmt,
                    stream.sample_aspect_ratio,
                    stream.avg_frame_rate,
                    stream.color_primaries,
                    stream.color_transfer,
                    stream.color_space,
                    stream.color_range,
                    plan.width,
                    plan.height,
                    plan.output_pixel_format,
                    plan.output_sar(),
                    plan.fps_num,
                    plan.fps_den,
                    primaries,
                    transfer,
                    matrix,
                    if plan.full_range { "pc" } else { "tv" },
                ),
                &self.video.path,
            ));
        }
        let mut args: Vec<OsString> = [
            "-v",
            "error",
            "-err_detect",
            "explode",
            "-select_streams",
            "0",
            "-count_frames",
            "-show_entries",
            "stream=nb_read_frames",
            "-of",
            "default=nokey=1:noprint_wrappers=1",
            "-i",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        args.push(self.video.path.as_os_str().to_owned());
        let result = supervisor::run_capture(
            &CommandSpec {
                executable: ffprobe.to_owned(),
                args,
                cwd: None,
            },
            cancel.clone(),
            64 * 1024,
            Duration::from_secs(24 * 60 * 60),
        )
        .await
        .map_err(|error| process_error(error, &self.video.path))?;
        let frames = String::from_utf8_lossy(&result.stdout)
            .trim()
            .parse::<usize>()
            .ok();
        if !result.status.success() || !result.stderr.is_empty() || frames != Some(expected_frames)
        {
            return Err(files::error(
                "QTGMC_PREPARE_FAILED",
                format!(
                    "The QTGMC intermediate did not decode to the expected {expected_frames} frames."
                ),
                &self.video.path,
            ));
        }
        Ok(())
    }

    /// Bind recovery to the decoded result of the full QTGMC/plugin pipeline,
    /// rather than only the VSPipe executable or Matroska container bytes.
    pub async fn decoded_identity(
        &self,
        ffmpeg: &Path,
        expected_frames: usize,
        cancel: &watch::Receiver<bool>,
    ) -> Result<String, AppError> {
        super::av1an_preprocess::decoded_identity(
            ffmpeg,
            &self.video.path,
            0,
            expected_frames,
            cancel,
        )
        .await
    }
}

pub(super) async fn check_tools(
    vspipe: &Path,
    environment: Option<&ChildEnvironment>,
    work: &Path,
    settings: QtgmcSettings,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    let program = format!(
        "import vapoursynth as vs\nimport havsfunc\ncore = vs.core\nclip = core.std.BlankClip(width=192, height=128, length=8, format=vs.YUV420P8, fpsnum=30000, fpsden=1001)\nclip = core.std.SetFrameProps(clip, _FieldBased={field})\nclip = havsfunc.QTGMC(clip, Preset={preset:?}, TFF={tff}, FPSDivisor={divisor})\nwith clip.get_frame(0):\n    print('JESSES_QTGMC_OK', clip.num_frames, clip.fps_num, clip.fps_den)\nclip.set_output()\n",
        field = if settings.field_order == FieldOrder::TopFirst {
            2
        } else {
            1
        },
        preset = preset(settings.preset),
        tff = if settings.field_order == FieldOrder::TopFirst {
            "True"
        } else {
            "False"
        },
        divisor = if settings.mode == DeinterlaceMode::Frame {
            2
        } else {
            1
        },
    );
    let script = Script::create(work, &program)?;
    let result = supervisor::run_capture_with_environment(
        &CommandSpec {
            executable: vspipe.to_owned(),
            args: vec![
                "--info".into(),
                script.path.as_os_str().to_owned(),
                OsString::from("-"),
            ],
            cwd: Some(work.to_owned()),
        },
        cancel.clone(),
        128 * 1024,
        Duration::from_secs(45),
        environment,
    )
    .await
    .map_err(|error| process_error(error, vspipe))?;
    let diagnostic = format!(
        "{}\n{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    if !result.status.success() || !diagnostic.contains("JESSES_QTGMC_OK") {
        return Err(files::error(
            "QTGMC_DEPENDENCY_MISSING",
            format!(
                "The selected VapourSynth runtime could not run QTGMC with havsfunc, MVTools, Misc, znedi3, RGVS and fmtconv. {}",
                diagnostic
                    .lines()
                    .rev()
                    .take(8)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            vspipe,
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_paths_are_literals_and_trim_precedes_qtgmc() {
        let script = source_script(
            Path::new("C:/media/a'b 日本語.mkv"),
            Path::new("C:/work/cache.lwi"),
            2,
            Some((4, 19)),
            QtgmcSettings {
                mode: DeinterlaceMode::Frame,
                field_order: FieldOrder::BottomFirst,
                preset: QtgmcPreset::Slow,
            },
        )
        .unwrap();
        assert!(script.contains("source=\"C:/media/a'b 日本語.mkv\""));
        assert!(script.contains("clip = clip[4:19]"));
        assert!(script.contains("Preset=\"Slow\", TFF=False, FPSDivisor=2"));
        assert!(script.find("clip = clip[4:19]").unwrap() < script.find("QTGMC").unwrap());
    }

    #[test]
    fn missing_av1an_allows_native_vspipe_fallback_but_discovery_errors_fail() {
        assert!(
            optional_av1an(Err(AppError::new("TOOL_MISSING", "absent", None)))
                .unwrap()
                .is_none()
        );
        let error = optional_av1an(Err(AppError::new(
            "TOOL_DISCOVERY_FAILED",
            "configured path is invalid",
            None,
        )))
        .unwrap_err();
        assert_eq!(error.code, "TOOL_DISCOVERY_FAILED");
    }

    #[tokio::test]
    #[ignore = "requires the managed av1an/VapourSynth QTGMC runtime"]
    async fn qtgmc_installed_runtime_executes_real_frames() {
        let (_sender, cancel) = watch::channel(false);
        let runtime = runtime(&cancel).await.unwrap();
        let work = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/qtgmc-capability-test");
        std::fs::create_dir_all(&work).unwrap();
        check_tools(
            &runtime.vspipe,
            runtime.environment.as_ref(),
            &work,
            QtgmcSettings {
                mode: DeinterlaceMode::Bob,
                field_order: FieldOrder::TopFirst,
                preset: QtgmcPreset::Fast,
            },
            &cancel,
        )
        .await
        .unwrap();
    }
}
