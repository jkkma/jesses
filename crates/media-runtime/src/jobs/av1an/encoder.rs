//! The av1an child command differs from each standalone command's I/O wrapper.
//! Reuse the validated standalone scalar arguments, then remove only the I/O
//! switches that av1an itself supplies for every chunk.
use super::*;
use media_core::VideoEncoder;

pub(super) fn name(encoder: VideoEncoder) -> &'static str {
    match encoder {
        VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => "svt-av1",
        VideoEncoder::X264 => "x264",
        _ => unreachable!("validated av1an encoder"),
    }
}

pub(super) fn receipt_name(encoder: VideoEncoder) -> &'static str {
    match encoder {
        VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => "svt_av1",
        _ => name(encoder),
    }
}

pub(super) fn binary(encoder: VideoEncoder) -> &'static str {
    match encoder {
        VideoEncoder::SvtAv1 | VideoEncoder::SvtAv1FiveFish | VideoEncoder::SvtAv1Hdr => {
            "SvtAv1EncApp"
        }
        VideoEncoder::X264 => "x264",
        _ => unreachable!("validated av1an encoder"),
    }
}

pub(super) fn chunk_extension(encoder: VideoEncoder) -> &'static str {
    match encoder {
        VideoEncoder::X264 => "264",
        _ => "ivf",
    }
}

pub(in crate::jobs) fn uses_mkvmerge(encoder: VideoEncoder, settings: &EncodeSettings) -> bool {
    encoder == VideoEncoder::X264
        || settings.av1an_options.unwrap_or_default().concat_method
            == media_core::Av1anConcatMethod::Mkvmerge
}

pub(super) fn video_extension(encoder: VideoEncoder, settings: &EncodeSettings) -> &'static str {
    if encoder == VideoEncoder::X264 || uses_mkvmerge(encoder, settings) {
        "mkv"
    } else {
        "ivf"
    }
}

fn without_pairs(mut args: Vec<OsString>, pairs: &[&str], singletons: &[&str]) -> Vec<OsString> {
    let mut index = 0;
    while index < args.len() {
        if pairs.iter().any(|option| args[index] == *option) {
            args.drain(index..(index + 2).min(args.len()));
        } else if singletons.iter().any(|option| args[index] == *option) {
            args.remove(index);
        } else {
            index += 1;
        }
    }
    args
}

pub(super) fn parameters(plan: &Plan, settings: &EncodeSettings) -> Vec<OsString> {
    let mut args = match settings.encoder {
        encoder if encoder.is_svt() => encoder_parameters(plan, settings),
        VideoEncoder::X264 => {
            let args = super::super::encode::standalone_x264_arguments(plan, settings);
            let args = without_pairs(
                args,
                &["--demuxer", "--muxer", "--fps", "-o"],
                &["--force-cfr", "-"],
            );
            // av1an's x264 driver emits raw Annex B and supplies --stitchable.
            args
        }
        _ => unreachable!("validated av1an encoder"),
    };
    super::grain::parameters(&mut args, settings);
    if let Some(threads) = settings.av1an_options.unwrap_or_default().encoder_threads {
        match settings.encoder {
            encoder if encoder.is_svt() => args.extend(["--lp".into(), threads.to_string().into()]),
            VideoEncoder::X264 => args.extend(["--threads".into(), threads.to_string().into()]),
            _ => unreachable!("validated av1an encoder"),
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::metadata::Document;

    #[test]
    fn x264_params_keep_validated_color_and_quality_without_standalone_io() {
        let document: Document = serde_json::from_value(serde_json::json!({
            "format":{"start_time":"0"},
            "streams":[{"index":0,"codec_type":"video","codec_name":"h264","width":128,"height":96,
                "pix_fmt":"yuv420p","sample_aspect_ratio":"1:1","avg_frame_rate":"24/1","start_time":"0",
                "chroma_location":"left","color_space":"bt709","color_primaries":"bt709",
                "color_transfer":"bt709","color_range":"tv"}]
        })).unwrap();
        let settings = EncodeSettings {
            backend: media_core::EncodeBackend::Av1an,
            encoder: VideoEncoder::X264,
            crf: 23,
            preset: 5,
            ..Default::default()
        };
        let plan = Plan::build(&document, &[&document.streams[0]], &settings).unwrap();
        let args = parameters(&plan, &settings);
        let args: Vec<_> = args.iter().map(|value| value.to_str().unwrap()).collect();
        assert!(args.windows(2).any(|pair| pair == ["--crf", "23"]));
        assert!(args.windows(2).any(|pair| pair == ["--preset", "medium"]));
        assert!(args.windows(2).any(|pair| pair == ["--colorprim", "bt709"]));
        assert!(
            !args
                .iter()
                .any(|value| ["--muxer", "--force-cfr", "--fps", "-o", "-"].contains(value))
        );
        assert_eq!(chunk_extension(settings.encoder), "264");
        assert_eq!(video_extension(settings.encoder, &settings), "mkv");
    }
}
