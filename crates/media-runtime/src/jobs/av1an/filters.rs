//! Custom pixel filters run through a native FFmpeg argv value. The standalone
//! producer applies them before encoding; av1an prepares one verified lossless
//! source so scene detection and quality references see the same pixels.
//! Timeline, geometry and file I/O belong to other typed controls.
use media_core::{AppError, EncodeSettings};

pub(in crate::jobs) fn validate(settings: &EncodeSettings) -> Result<(), AppError> {
    let filters = &settings.av1an_filters;
    if settings.source_filters {
        return if filters.iter().any(|filter| filter.contains('\0')) {
            Err(AppError::new(
                "VIDEO_FILTER_INVALID",
                "An FFmpeg filter cannot contain a NUL character.",
                None,
            ))
        } else {
            Ok(())
        };
    }
    if filters.is_empty() {
        return Ok(());
    }
    let invalid = || {
        AppError::new(
            "VIDEO_FILTER_INVALID",
            "Use up to 16 pixel-filter rows (4096 characters each). Geometry, timing, external file access and filter graphs must use the dedicated controls.",
            None,
        )
    };
    if filters.len() > 16 || filters.iter().map(String::len).sum::<usize>() > 16384 {
        return Err(invalid());
    }
    for filter in filters {
        if filter.is_empty()
            || filter.len() > 4096
            || filter
                .chars()
                .any(|c| c.is_control() || matches!(c, ';' | '[' | ']'))
        {
            return Err(invalid());
        }
        let mut quoted = false;
        let mut escaped = false;
        let mut start = 0;
        let mut parts = Vec::new();
        for (index, c) in filter.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            if c == '\\' {
                escaped = true;
                continue;
            }
            if c == '\'' {
                quoted = !quoted;
            }
            if c == ',' && !quoted {
                parts.push(&filter[start..index]);
                start = index + 1;
            }
        }
        if quoted || escaped {
            return Err(invalid());
        }
        parts.push(&filter[start..]);
        for part in parts {
            let name = part.trim().split('=').next().unwrap_or_default();
            if !matches!(
                name,
                "eq" | "unsharp"
                    | "hqdn3d"
                    | "nlmeans"
                    | "atadenoise"
                    | "bm3d"
                    | "deband"
                    | "gradfun"
                    | "deblock"
                    | "cas"
                    | "noise"
                    | "vibrance"
                    | "colorbalance"
                    | "colorlevels"
                    | "curves"
                    | "hue"
                    | "lut"
                    | "lutyuv"
                    | "lutrgb"
                    | "limiter"
                    | "negate"
                    | "normalize"
                    | "smartblur"
                    | "gblur"
                    | "boxblur"
                    | "chromanr"
            ) {
                return Err(invalid());
            }
            if part.contains("psfile")
                || part.contains("file=")
                || (name == "curves" && part.contains("plot"))
            {
                return Err(invalid());
            }
            if name == "curves" {
                // Curves also accepts file paths as positional options. Permit
                // only named pixel controls so those slots cannot be reached.
                let Some((_, options)) = part.split_once('=') else {
                    continue;
                };
                if options.split(':').any(|option| {
                    option.split_once('=').is_none_or(|(key, _)| {
                        !matches!(
                            key.trim(),
                            "preset"
                                | "master"
                                | "red"
                                | "green"
                                | "blue"
                                | "all"
                                | "r"
                                | "g"
                                | "b"
                                | "m"
                                | "interp"
                        )
                    })
                }) {
                    return Err(invalid());
                }
            }
        }
    }
    Ok(())
}

/// The source keeps each row verbatim and strips only blank/short chain entries.
pub(in crate::jobs) fn rows(settings: &EncodeSettings) -> Vec<String> {
    settings
        .av1an_filters
        .iter()
        .filter(|filter| !settings.source_filters || filter.trim().encode_utf16().count() > 2)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use media_core::EncodeBackend;
    #[test]
    fn permits_pixel_expressions_but_rejects_hidden_io_and_timeline_changes() {
        let mut settings = EncodeSettings {
            av1an_filters: vec![
                "eq=contrast=1.1:saturation=0.9".into(),
                "unsharp=5:5:0.4".into(),
                "hue=h='if(lt(t,10),0,20)'".into(),
            ],
            ..Default::default()
        };
        assert!(validate(&settings).is_ok());
        settings.backend = EncodeBackend::Standalone;
        assert!(validate(&settings).is_ok());
        for filter in [
            "movie=/tmp/input",
            "eq=contrast=1.1,trim=0:1",
            "curves=psfile=/tmp/file",
            "curves=plot=/tmp/file",
            "curves=none::::::/tmp/file",
            "eq=1;null[out]",
            "eq=1\n",
            "hue=h='broken",
        ] {
            settings.av1an_filters = vec![filter.into()];
            assert!(validate(&settings).is_err(), "{filter}");
        }
    }

    #[test]
    fn source_rows_preserve_arbitrary_graphs_order_and_verbatim_values() {
        let mut settings = EncodeSettings {
            source_filters: true,
            av1an_filters: vec![
                " ".into(),
                "ab".into(),
                "  [in]scale=80:48,setsar=2[out]  ".into(),
                "fps=12,tmix=frames=3".into(),
                "drawtext=text='a b':x=10".into(),
            ],
            ..Default::default()
        };
        validate(&settings).unwrap();
        assert_eq!(rows(&settings), settings.av1an_filters[2..]);
        settings.av1an_filters = vec!["éé".into(), "🧪".into(), "negate".into()];
        assert_eq!(rows(&settings), vec!["negate".to_owned()]);
        settings.av1an_filters = std::iter::repeat_n("negate".into(), 20).collect();
        validate(&settings).unwrap();
        settings.av1an_filters.push("hue=\0".into());
        assert!(validate(&settings).is_err());
        settings.source_filters = false;
        settings.av1an_filters = vec!["scale=80:48".into()];
        assert!(validate(&settings).is_err());
    }
}
