//! Custom pixel filters run through a native FFmpeg argv value on the verified
//! lossless preparation path. Timeline, geometry and file I/O belong to other
//! typed controls and cannot be hidden in a filter expression.
use media_core::{AppError, EncodeBackend, EncodeSettings};

pub(in crate::jobs) fn validate(settings: &EncodeSettings) -> Result<(), AppError> {
    let filters = &settings.av1an_filters;
    if filters.is_empty() {
        return Ok(());
    }
    let invalid = || {
        AppError::new(
            "AV1AN_FILTER_INVALID",
            "Use up to 16 pixel-filter rows (4096 characters each). Geometry, timing, external file access and filter graphs must use the dedicated controls.",
            None,
        )
    };
    if settings.backend != EncodeBackend::Av1an
        || filters.len() > 16
        || filters.iter().map(String::len).sum::<usize>() > 16384
    {
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permits_pixel_expressions_but_rejects_hidden_io_and_timeline_changes() {
        let mut settings = EncodeSettings {
            backend: EncodeBackend::Av1an,
            av1an_filters: vec![
                "eq=contrast=1.1:saturation=0.9".into(),
                "unsharp=5:5:0.4".into(),
                "hue=h='if(lt(t,10),0,20)'".into(),
            ],
            ..Default::default()
        };
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
}
