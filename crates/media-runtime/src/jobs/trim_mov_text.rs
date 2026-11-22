//! Trim MOV_TEXT samples on the picture clock. Copying their compressed bytes
//! keeps tx3g style runs and other in-band data intact.
use super::{Interval, PictureClock, unsupported};
use crate::{
    jobs::{
        container, external_tracks::ExternalTracks, files::Temporary, metadata::Stream,
        process_error, trim_subtitle_verify::TrackProof,
    },
    supervisor::{self, CommandSpec},
};
use media_core::{AppError, EncodeSettings, SubtitleMode};
use std::{
    ffi::OsString,
    ops::Range,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::sync::watch;

#[derive(Default)]
pub(super) struct PreparedTracks {
    pub assets: Vec<(u32, PathBuf)>,
    pub proof: Vec<(u32, TrackProof)>,
}

fn bounds(
    picture: PictureClock,
    interval: Interval,
    track_origin: f64,
) -> Result<Range<i64>, AppError> {
    let (numerator, denominator) = picture.rate;
    let instant = |frame: usize| {
        let offset = frame as f64 * f64::from(denominator) / f64::from(numerator);
        let ticks = ((track_origin + offset) * 1_000_000.0).round();
        // FFmpeg's bitstream expressions represent integers exactly through 2^53.
        if !ticks.is_finite() || ticks.abs() >= (1_u64 << 53) as f64 {
            return Err(unsupported(
                "The copied MOV_TEXT trim clock is out of range.",
            ));
        }
        Ok(ticks as i64)
    };
    Ok(instant(interval.source_start_frame)?..instant(interval.source_end_frame)?)
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn prepare(
    settings: &EncodeSettings,
    interval: Interval,
    picture: PictureClock,
    selected: &[&Stream],
    external: &ExternalTracks,
    ffmpeg: &Path,
    ffprobe: &Path,
    input: &Path,
    output: &Path,
    id: &str,
    scratch: &mut Vec<Temporary>,
    cancel: &watch::Receiver<bool>,
) -> Result<PreparedTracks, AppError> {
    let mut prepared = PreparedTracks::default();
    if container::source_iso_format(output, selected, settings).is_none() {
        return Ok(prepared);
    }
    for stream in selected.iter().filter(|stream| {
        stream.codec_type.as_deref() == Some("subtitle")
            && stream.codec_name.as_deref() == Some("mov_text")
            && settings
                .subtitles
                .iter()
                .find(|track| track.stream_index == stream.index)
                .is_none_or(|track| {
                    matches!(track.mode, SubtitleMode::Copy | SubtitleMode::MovText)
                })
    }) {
        let (path, index, origin) =
            if let Some((path, document, index)) = external.source_track(stream.index) {
                let start = document
                    .format
                    .as_ref()
                    .and_then(|format| format.start_time.as_deref())
                    .and_then(|start| start.parse::<f64>().ok())
                    .unwrap_or(0.0);
                (path, index, start - external.offset_seconds(stream.index))
            } else {
                (input, stream.index, picture.origin)
            };
        let window = bounds(picture, interval, origin)?;
        let proof =
            TrackProof::read(ffprobe, path, index, window.start, window.end, cancel).await?;
        let asset = make_asset(
            ffmpeg,
            path,
            index,
            output,
            &format!("{id}-mov-text-{}", stream.index),
            window,
            proof.cues.is_empty(),
            cancel,
        )
        .await?;
        prepared.assets.push((stream.index, asset.path.clone()));
        prepared.proof.push((stream.index, proof));
        scratch.push(asset);
    }
    Ok(prepared)
}

#[allow(clippy::too_many_arguments)]
async fn make_asset(
    ffmpeg: &Path,
    path: &Path,
    index: u32,
    output: &Path,
    id: &str,
    window: Range<i64>,
    empty: bool,
    cancel: &watch::Receiver<bool>,
) -> Result<Temporary, AppError> {
    let asset = Temporary::create_extension(output, id, "mp4")?;
    let mut args = copy_args(path, index, true);
    if empty {
        // An MP4 muxer omits an unwritten track. Move one source sample to
        // the clip end to make a leading clearing sample with its codec
        // header, then keep only that clear. No dialogue enters the final mux.
        let duration = window.end - window.start;
        let seed = Temporary::create_extension(output, &format!("{id}-blank-seed"), "mp4")?;
        let filter = format!(
            "setts=pts={duration}:dts={duration}:duration=1:time_base=1/1000000:prescale=1"
        );
        args.extend([
            "-frames:s".into(),
            "1".into(),
            "-bsf:s".into(),
            filter.into(),
        ]);
        mux(ffmpeg, args, &seed, cancel).await?;
        args = copy_args(&seed.path, 0, false);
        args.extend(["-bsf:s".into(), "noise=amount=0:drop='gt(pts,0)'".into()]);
        mux(ffmpeg, args, &asset, cancel).await?;
    } else {
        let Range { start, end } = window;
        let filter = format!(
            "setts=pts='max(PTS-({start}),0)':dts='max(DTS-({start}),0)':duration='max(min(PTS+DURATION,{end})-max(PTS,{start}),0)':time_base=1/1000000:prescale=1,noise=amount=0:drop='lte(duration,0)'"
        );
        args.extend(["-bsf:s".into(), filter.into()]);
        if end > 0 {
            args.extend([
                "-to".into(),
                format!("{:.6}", end as f64 / 1_000_000.0).into(),
            ]);
        }
        mux(ffmpeg, args, &asset, cancel).await?;
    }
    Ok(asset)
}

fn copy_args(path: &Path, index: u32, discard_other_kinds: bool) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-xerror",
        "-copyts",
        "-protocol_whitelist",
        "file",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    if discard_other_kinds {
        // With other media kinds discarded, the MOV demuxer can seek between
        // subtitle samples without reading the intervening video/audio bytes.
        // Start at the first subtitle sample to retain any cue already active
        // at the cut, regardless of which preceding video frame is seekable.
        for kind in ["v", "a", "d", "t"] {
            args.extend([format!("-discard:{kind}").into(), "all".into()]);
        }
    }
    args.extend([
        "-i".into(),
        path.as_os_str().to_owned(),
        "-map".into(),
        format!("0:{index}").into(),
        "-c".into(),
        "copy".into(),
        "-copypriorss".into(),
        "1".into(),
        "-map_metadata".into(),
        "-1".into(),
        "-map_chapters".into(),
        "-1".into(),
    ]);
    args
}

async fn mux(
    ffmpeg: &Path,
    mut args: Vec<OsString>,
    asset: &Temporary,
    cancel: &watch::Receiver<bool>,
) -> Result<(), AppError> {
    args.extend(["-y".into(), asset.path.as_os_str().to_owned()]);
    let result = supervisor::run_capture(
        &CommandSpec {
            executable: ffmpeg.to_owned(),
            args,
            cwd: None,
        },
        cancel.clone(),
        64 * 1024,
        Duration::from_secs(60 * 60),
    )
    .await
    .map_err(|error| process_error(error, &asset.path))?;
    if !result.status.success() {
        let diagnostic = String::from_utf8_lossy(&result.stderr);
        return Err(unsupported(format!(
            "Could not clip the copied MOV_TEXT track: {}",
            diagnostic.trim()
        )));
    }
    asset.flush_nonempty_async().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_ordinals_keep_nonzero_origin_and_fractional_frames() {
        let trim = serde_json::from_value(serde_json::json!({
            "startFrame": 17, "endFrameExclusive": 41,
        }))
        .unwrap();
        let interval = Interval::build(trim, 72, 24_000, 1_001).unwrap();
        let clock = PictureClock {
            origin: 2.002,
            rate: (24_000, 1_001),
        };
        assert_eq!(
            bounds(clock, interval, clock.origin).unwrap(),
            2_711_042..3_712_042
        );
        assert_eq!(bounds(clock, interval, -0.25).unwrap(), 459_042..1_460_042);
    }
}
