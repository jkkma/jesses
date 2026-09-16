//! Explicit linear-light HDR/HLG rendering, with a separate SDR output contract.
use media_core::{AppError, EncodeSettings, ToneMapSettings};
use std::{ffi::OsString, path::Path, time::Duration};
use tokio::sync::watch;

use super::{Stream, unsupported};
use crate::supervisor::{self, CommandSpec};

pub(super) fn validate_settings(settings: &EncodeSettings) -> Result<(), AppError> {
    if let Some(tone) = settings.tone_map
        && (!(100..=10_000).contains(&tone.source_peak_nits) || settings.hdr10_fallback)
    {
        return Err(unsupported(
            "Tone mapping requires a signal peak from 100 to 10000 nits. Use its separate HDR10 base-layer option instead of HDR10 output fallback.",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub(super) struct Transform {
    pub settings: ToneMapSettings,
    pub hlg: bool,
    chroma: &'static str,
}

impl Transform {
    pub fn build(video: &Stream, settings: &EncodeSettings) -> Result<Option<Self>, AppError> {
        let Some(tone) = settings.tone_map else {
            return Ok(None);
        };
        let hlg = video.color_transfer.as_deref() == Some("arib-std-b67");
        if !matches!(
            video.color_transfer.as_deref(),
            Some("smpte2084" | "arib-std-b67")
        ) || video.color_primaries.as_deref() != Some("bt2020")
            || video.color_space.as_deref() != Some("bt2020nc")
            || video.color_range.as_deref() != Some("tv")
            || video.pix_fmt.as_deref() != Some("yuv420p10le")
        {
            return Err(unsupported(
                "Tone mapping requires explicitly tagged limited-range 10-bit 4:2:0 BT.2020 PQ/HDR10 or HLG video. SDR, missing tags and other HDR representations are not silently reinterpreted.",
            ));
        }
        let chroma = match video.chroma_location.as_deref() {
            Some("left") => "left",
            Some("topleft") => "topleft",
            Some("center") => "center",
            _ => {
                return Err(unsupported(
                    "Tone mapping requires explicit left, center or top-left source chroma placement.",
                ));
            }
        };
        if hlg && tone.hdr10_base_layer {
            return Err(unsupported(
                "HDR10 base-layer fallback cannot be applied to HLG.",
            ));
        }
        Ok(Some(Self {
            settings: tone,
            hlg,
            chroma,
        }))
    }

    pub fn filter(&self) -> String {
        let transfer = if self.hlg {
            "arib-std-b67"
        } else {
            "smpte2084"
        };
        // zimg maps PQ absolute luminance (and its 1000-nit reference HLG EOTF)
        // to linear values relative to npl=100. Both tonemap input and its
        // explicit signal peak therefore use the same 100-nit units.
        // tonemap reads matrix tags for desaturation even on RGB planes. GBR's
        // coefficients sum all channels, so tag the physical BT.709 primaries
        // for its luma calculation; tell final zscale explicitly that the
        // samples are still linear RGB. Neutral-ramp arithmetic tests guard it.
        format!(
            "zscale=pin=bt2020:tin={transfer}:min=bt2020nc:rin=limited:cin={}:p=bt2020:t=linear:m=gbr:r=full:npl=100:agamma=0,format=gbrpf32le,zscale=p=bt709,setparams=colorspace=bt709,tonemap=tonemap=hable:desat=2:peak={:.2},zscale=pin=bt709:tin=linear:min=gbr:rin=full:p=bt709:t=bt709:m=bt709:r=limited:c=left:dither=error_diffusion:agamma=0,format=yuv420p10le,limiter=min=64:max=940:planes=1,limiter=min=64:max=960:planes=6,sidedata=mode=delete,setparams=field_mode=prog:range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709",
            self.chroma,
            f64::from(self.settings.source_peak_nits) / 100.0
        )
    }

    pub async fn check_tools(
        &self,
        ffmpeg: &Path,
        cancel: &watch::Receiver<bool>,
    ) -> Result<(), AppError> {
        let filter = format!("format=yuv420p10le,{}", self.filter());
        let mut args: Vec<OsString> = [
            "-v",
            "error",
            "-nostdin",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=64x64:r=1,format=yuv420p10le",
            "-frames:v",
            "1",
            "-vf",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        args.extend([
            filter.into(),
            "-pix_fmt".into(),
            "+yuv420p10le".into(),
            "-f".into(),
            "rawvideo".into(),
            "pipe:1".into(),
        ]);
        let captured = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg.to_owned(),
                args,
                cwd: None,
            },
            cancel.clone(),
            128 * 1024,
            Duration::from_secs(10),
        )
        .await
        .map_err(|error| crate::jobs::process_error(error, ffmpeg))?;
        if !captured.status.success() || captured.stdout.len() != 64 * 64 * 3 {
            return Err(AppError::new(
                "TONE_MAP_TOOL_UNSUPPORTED",
                format!(
                    "The installed FFmpeg could not run the required linear-light zscale/Hable pipeline: {}",
                    String::from_utf8_lossy(&captured.stderr)
                ),
                None,
            ));
        }
        Ok(())
    }
}
