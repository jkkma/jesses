//! Bounded CSV/SVG exports of immutable analysis snapshots, with no source reads.
use super::files::{self, Temporary};
use media_core::{
    AnalysisExportFormat, AnalysisExportRequest, AnalysisReport, AppError, QualityMetric,
};
use std::{
    fmt::Write as _,
    io::Write as _,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_REPORT: AtomicU64 = AtomicU64::new(1);
const MAX_REPORT_BYTES: usize = 16 * 1024 * 1024;

fn invalid(message: &str) -> AppError {
    AppError::new("ANALYSIS_EXPORT_INVALID", message, None)
}
fn text_valid(text: &str, limit: usize) -> bool {
    text.len() <= limit && !text.chars().any(|c| c.is_control())
}
fn fingerprint(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|c| c.is_ascii_hexdigit())
}
fn finite(value: f64) -> bool {
    value.is_finite() && value.abs() <= 1e18
}
fn decimal(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()) && value.parse::<u64>().is_ok()
}
fn validate(report: &AnalysisReport) -> Result<(), AppError> {
    let okay = match report {
        AnalysisReport::Bitrate { request, result } => {
            text_valid(&request.input_path, 32768)
                && !request.input_path.is_empty()
                && request.stream_index == result.stream_index
                && request.window_seconds == result.window_seconds
                && finite(result.window_seconds)
                && result.window_seconds > 0.0
                && result.points.len() <= 7200
                && fingerprint(&result.source_fingerprint)
                && [
                    &result.packet_bytes,
                    &result.packet_count,
                    &result.untimed_packet_bytes,
                    &result.untimed_packet_count,
                    &result.dts_fallback_count,
                ]
                .into_iter()
                .all(|v| decimal(v))
                && [
                    result.start_seconds,
                    result.end_seconds,
                    result.average_megabits_per_second,
                ]
                .into_iter()
                .flatten()
                .all(finite)
                && finite(result.peak_window_megabits_per_second)
                && result.points.iter().all(|p| {
                    finite(p.start_seconds)
                        && finite(p.megabits_per_second)
                        && p.megabits_per_second >= 0.0
                        && decimal(&p.packet_bytes)
                })
                && result
                    .points
                    .windows(2)
                    .all(|p| p[0].start_seconds < p[1].start_seconds)
        }
        AnalysisReport::Quality { request, result } => {
            [&request.reference_path, &request.candidate_path]
                .into_iter()
                .all(|p| !p.is_empty() && text_valid(p, 32768))
                && request.metric == result.metric
                && request.frame_count == result.frame_count
                && (1..=60000).contains(&result.frame_count)
                && result.points.len() == result.frame_count as usize
                && fingerprint(&result.reference_fingerprint)
                && fingerprint(&result.candidate_fingerprint)
                && result.model.as_ref().is_none_or(|v| text_valid(v, 128))
                && text_valid(&result.message, 4096)
                && result.score.is_none_or(finite)
                && (result.score.is_some() || result.metric == QualityMetric::Psnr)
                && result.points.iter().enumerate().all(|(i, p)| {
                    p.frame as usize == i
                        && p.score.is_none_or(finite)
                        && (p.score.is_some() || result.metric == QualityMetric::Psnr)
                })
                && request
                    .reference_start_frame
                    .checked_add(result.frame_count)
                    .is_some()
                && request
                    .candidate_start_frame
                    .checked_add(result.frame_count)
                    .is_some()
        }
    };
    if okay {
        Ok(())
    } else {
        Err(invalid(
            "The analysis snapshot is invalid or exceeds the export limit. Run the analysis again.",
        ))
    }
}

fn metric_name(metric: QualityMetric) -> &'static str {
    match metric {
        QualityMetric::Ssim => "SSIM",
        QualityMetric::Psnr => "PSNR (dB)",
        QualityMetric::Vmaf => "VMAF",
    }
}
fn score(value: Option<f64>) -> String {
    value.map_or_else(|| "infinity".into(), |v| v.to_string())
}
fn metadata(report: &AnalysisReport) -> Vec<(&'static str, String)> {
    match report {
        AnalysisReport::Bitrate { request, result } => vec![
            ("analysis","Packet bitrate".into()),("source_path",request.input_path.clone()),
            ("source_fingerprint",result.source_fingerprint.clone()),("stream_index",result.stream_index.to_string()),
            ("window_seconds",result.window_seconds.to_string()),("packet_bytes",result.packet_bytes.clone()),
            ("packet_count",result.packet_count.clone()),("untimed_packet_bytes",result.untimed_packet_bytes.clone()),
            ("untimed_packet_count",result.untimed_packet_count.clone()),("dts_fallback_count",result.dts_fallback_count.clone()),
            ("average_megabits_per_second",result.average_megabits_per_second.map_or_else(|| "unknown".into(),|v|v.to_string())),
            ("peak_window_megabits_per_second",result.peak_window_megabits_per_second.to_string()),
            ("payload_scope","Compressed packets only; container overhead excluded. Windows use their full width.".into())],
        AnalysisReport::Quality { request, result } => vec![
            ("analysis",metric_name(result.metric).into()),("reference_path",request.reference_path.clone()),
            ("candidate_path",request.candidate_path.clone()),("reference_fingerprint",result.reference_fingerprint.clone()),
            ("candidate_fingerprint",result.candidate_fingerprint.clone()),("reference_stream_index",request.reference_stream_index.to_string()),
            ("candidate_stream_index",request.candidate_stream_index.to_string()),("reference_start_frame",request.reference_start_frame.to_string()),
            ("candidate_start_frame",request.candidate_start_frame.to_string()),("frame_count",result.frame_count.to_string()),
            ("aggregate_score",score(result.score)),("model",result.model.clone().unwrap_or_default()),
            ("pairing","Explicit corresponding frame intervals. Scores apply only to this interval.".into())],
    }
}

fn csv_cell(value: &str) -> String {
    // Text remains text when opened by a spreadsheet; numeric columns are emitted separately.
    let prefix = if value.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        "'"
    } else {
        ""
    };
    format!("\"{prefix}{}\"", value.replace('"', "\"\""))
}
fn csv(report: &AnalysisReport) -> String {
    let mut output = String::from("\u{feff}field,value\r\n");
    for (key, value) in metadata(report) {
        writeln!(output, "{},{}\r", csv_cell(key), csv_cell(&value)).unwrap();
    }
    output.push_str("\r\n");
    match report {
        AnalysisReport::Bitrate { result, .. } => {
            output.push_str(
                "window_start_seconds,window_end_seconds,megabits_per_second,packet_bytes\r\n",
            );
            for p in &result.points {
                writeln!(
                    output,
                    "{},{},{},{}\r",
                    p.start_seconds,
                    p.start_seconds + result.window_seconds,
                    p.megabits_per_second,
                    p.packet_bytes
                )
                .unwrap();
            }
        }
        AnalysisReport::Quality { request, result } => {
            output.push_str("comparison_frame,reference_frame,candidate_frame,score\r\n");
            for p in &result.points {
                writeln!(
                    output,
                    "{},{},{},{}\r",
                    p.frame,
                    request.reference_start_frame + p.frame,
                    request.candidate_start_frame + p.frame,
                    score(p.score)
                )
                .unwrap();
            }
        }
    }
    output
}
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn svg(report: &AnalysisReport) -> String {
    let (title, x_label, y_label, points, summary) = match report {
        AnalysisReport::Bitrate { result, .. } => (
            format!("Packet bitrate · stream #{}", result.stream_index),
            "Source presentation time (seconds)",
            "Mb/s",
            result
                .points
                .iter()
                .map(|p| (p.start_seconds, Some(p.megabits_per_second)))
                .collect::<Vec<_>>(),
            format!(
                "{} s windows · {} packets · compressed payload only",
                result.window_seconds, result.packet_count
            ),
        ),
        AnalysisReport::Quality { result, .. } => (
            format!("{} by comparison frame", metric_name(result.metric)),
            "Comparison frame (zero-based)",
            metric_name(result.metric),
            result
                .points
                .iter()
                .map(|p| (f64::from(p.frame), p.score))
                .collect(),
            format!(
                "Aggregate: {} · {} frames · {}",
                result
                    .score
                    .map_or_else(|| "infinity".into(), |value| format!("{value:.6}")),
                result.frame_count,
                result
                    .model
                    .as_deref()
                    .unwrap_or("explicit matching intervals")
            ),
        ),
    };
    let xmin = points.first().map_or(0.0, |p| p.0);
    let mut xmax = points.last().map_or(1.0, |p| p.0);
    if xmax - xmin < 1e-9 {
        xmax = xmin + (xmin.abs() * 1e-6).max(1.0);
    }
    let mut ymin: f64 = points
        .iter()
        .filter_map(|p| p.1)
        .fold(f64::INFINITY, f64::min);
    let mut ymax: f64 = points
        .iter()
        .filter_map(|p| p.1)
        .fold(f64::NEG_INFINITY, f64::max);
    if !ymin.is_finite() {
        ymin = 0.0;
        ymax = 1.0;
    }
    if matches!(report, AnalysisReport::Bitrate { .. }) {
        ymin = 0.0;
    }
    if ymax - ymin < 1e-9 {
        ymax = ymin + (ymin.abs() * 1e-6).max(1.0);
    }
    let mut output = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1200\" height=\"720\" viewBox=\"0 0 1200 720\" role=\"img\"><title>{}</title><desc>{}</desc><metadata>{}</metadata><rect width=\"1200\" height=\"720\" fill=\"#fff\"/><g font-family=\"sans-serif\" fill=\"#202020\"><text x=\"90\" y=\"45\" font-size=\"25\">{}</text><text x=\"90\" y=\"75\" font-size=\"15\">{}</text>",
        xml(&title),
        xml(&summary),
        xml(&serde_json::to_string(&metadata(report)).unwrap()),
        xml(&title),
        xml(&summary)
    );
    for tick in 0..=4 {
        let fraction = f64::from(tick) / 4.0;
        let x = 90.0 + 1050.0 * fraction;
        let y = 585.0 - 450.0 * fraction;
        write!(output,"<path d=\"M90 {y:.2}H1140\" stroke=\"#ddd\"/><text x=\"78\" y=\"{:.2}\" text-anchor=\"end\" font-size=\"13\">{:.4}</text><text x=\"{x:.2}\" y=\"610\" text-anchor=\"middle\" font-size=\"13\">{:.3}</text>",y+4.0,ymin+(ymax-ymin)*fraction,xmin+(xmax-xmin)*fraction).unwrap();
    }
    let mut path = String::new();
    let mut infinity = String::new();
    let mut connected = false;
    let mut single_marker = String::new();
    for (index, &(x, value)) in points.iter().enumerate() {
        let x = 90.0 + 1050.0 * (x - xmin) / (xmax - xmin);
        if let Some(value) = value {
            let y = 585.0 - 450.0 * (value - ymin) / (ymax - ymin);
            write!(path, "{}{x:.3},{y:.3}", if connected { 'L' } else { 'M' }).unwrap();
            if !connected && points.get(index + 1).is_none_or(|point| point.1.is_none()) {
                write!(
                    single_marker,
                    "<circle cx=\"{x:.3}\" cy=\"{y:.3}\" r=\"3\" fill=\"#a64b20\"/>"
                )
                .unwrap();
            }
            connected = true;
        } else {
            write!(infinity, "M{x:.3},119v7").unwrap();
            connected = false;
        }
    }
    write!(output,"<path d=\"M90 135V585H1140\" fill=\"none\" stroke=\"#555\"/><path d=\"{path}\" fill=\"none\" stroke=\"#a64b20\" stroke-width=\"1.5\"/>").unwrap();
    output.push_str(&single_marker);
    if !infinity.is_empty() {
        write!(output,"<path d=\"{infinity}\" stroke=\"#3257a8\"/><text x=\"90\" y=\"110\" font-size=\"13\">∞: identical decoded pixels (gaps in the finite curve)</text>").unwrap();
    }
    write!(output,"<text x=\"615\" y=\"648\" text-anchor=\"middle\" font-size=\"17\">{}</text><text transform=\"translate(22 360) rotate(-90)\" text-anchor=\"middle\" font-size=\"17\">{}</text><text x=\"90\" y=\"686\" font-size=\"13\">All measured points retained. Source identities and interval details are embedded in SVG metadata.</text></g></svg>",xml(x_label),xml(y_label)).unwrap();
    output
}

pub fn export_analysis(request: AnalysisExportRequest) -> Result<String, AppError> {
    validate(&request.report)?;
    let path = Path::new(&request.output_path);
    let extension = match request.format {
        AnalysisExportFormat::Csv => "csv",
        AnalysisExportFormat::Svg => "svg",
    };
    if !text_valid(&request.output_path, 32768)
        || !path.is_absolute()
        || !path
            .extension()
            .is_some_and(|v| v.eq_ignore_ascii_case(extension))
    {
        return Err(invalid(
            "Choose an absolute output path with the selected .csv or .svg extension.",
        ));
    }
    let parent = path
        .parent()
        .ok_or_else(|| invalid("Choose an existing output folder."))?;
    let parent = std::fs::canonicalize(parent)
        .map_err(|e| files::error("ANALYSIS_EXPORT_FAILED", e.to_string(), parent))?;
    let output = parent.join(
        path.file_name()
            .ok_or_else(|| invalid("Choose an output filename."))?,
    );
    files::ensure_absent(&output)?;
    let contents = match request.format {
        AnalysisExportFormat::Csv => csv(&request.report),
        AnalysisExportFormat::Svg => svg(&request.report),
    };
    if contents.len() > MAX_REPORT_BYTES {
        return Err(invalid("The report exceeds the 16 MiB export limit."));
    }
    let id = format!(
        "analysis-{}-{}",
        std::process::id(),
        NEXT_REPORT.fetch_add(1, Ordering::Relaxed)
    );
    let mut temporary = Temporary::create_extension(&output, &id, extension)?;
    let mut file = temporary.clone_file()?;
    file.write_all(contents.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|e| files::error("ANALYSIS_EXPORT_FAILED", e.to_string(), &output))?;
    drop(file);
    temporary.publish(&output)?;
    temporary.cleanup()?;
    Ok(output.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use media_core::{QualityPoint, QualityRequest, QualityResult};
    fn report() -> AnalysisReport {
        AnalysisReport::Quality {
            request: QualityRequest {
                reference_path: "=unsafe<source>\".mkv".into(),
                reference_stream_index: 4,
                reference_start_frame: 120,
                candidate_path: "candidate.mkv".into(),
                candidate_stream_index: 2,
                candidate_start_frame: 12,
                frame_count: 3,
                metric: QualityMetric::Psnr,
            },
            result: QualityResult {
                metric: QualityMetric::Psnr,
                frame_count: 3,
                score: Some(43.1),
                points: vec![
                    QualityPoint {
                        frame: 0,
                        score: Some(41.0),
                    },
                    QualityPoint {
                        frame: 1,
                        score: None,
                    },
                    QualityPoint {
                        frame: 2,
                        score: Some(45.0),
                    },
                ],
                reference_fingerprint: "a".repeat(64),
                candidate_fingerprint: "b".repeat(64),
                model: None,
                message: "Matched interval".into(),
            },
        }
    }
    #[test]
    fn reports_keep_source_identity_offsets_infinite_scores_and_xml_safety() {
        let report = report();
        validate(&report).unwrap();
        let csv = csv(&report);
        assert!(csv.contains("\"'=unsafe<source>\"\".mkv\""));
        assert!(csv.contains("1,121,13,infinity\r\n"));
        let svg = svg(&report);
        assert!(svg.contains("&lt;source&gt;\\&quot;"));
        assert!(!svg.contains("<source>"));
        assert!(svg.contains("gaps in the finite curve"));
        assert!(svg.contains("M90.000,585.000M1140.000,135.000"));
        assert!(svg.contains("<circle cx=\"90.000\" cy=\"585.000\""));
        assert!(svg.contains("<circle cx=\"1140.000\" cy=\"135.000\""));
        assert_eq!(svg.matches("<circle ").count(), 2);
    }
    #[test]
    fn saved_analysis_json_retains_the_measured_float_bits() {
        // Real pooled VMAF result: default approximate JSON parsing changes its
        // least significant bit before an otherwise lossless CSV/SVG export.
        let mut value = serde_json::to_value(report()).unwrap();
        value["result"]["score"] = serde_json::Value::from(97.09981280000007_f64);
        let encoded = serde_json::to_string(&value).unwrap();
        let parsed: AnalysisReport = serde_json::from_str(&encoded).unwrap();
        let AnalysisReport::Quality { result, .. } = &parsed else {
            panic!("expected quality report");
        };
        assert_eq!(
            result.score.unwrap().to_bits(),
            97.09981280000007_f64.to_bits()
        );
        assert!(csv(&parsed).contains("97.09981280000007"));
        assert!(svg(&parsed).contains("97.09981280000007"));
    }
    #[test]
    fn publication_never_replaces_existing_files_and_cleans_temporaries() {
        let directory = std::env::temp_dir().join(format!(
            "jesses-report-test-{}-{}",
            std::process::id(),
            NEXT_REPORT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&directory).unwrap();
        let output = directory.join("report.csv");
        let request = AnalysisExportRequest {
            output_path: output.to_string_lossy().into(),
            format: AnalysisExportFormat::Csv,
            report: report(),
        };
        export_analysis(request.clone()).unwrap();
        let before = std::fs::read(&output).unwrap();
        assert_eq!(export_analysis(request).unwrap_err().code, "OUTPUT_EXISTS");
        assert_eq!(std::fs::read(&output).unwrap(), before);
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 1);
        std::fs::remove_file(output).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
    #[test]
    fn bitrate_export_keeps_full_windows_and_large_decimal_payloads() {
        let value = AnalysisReport::Bitrate {
            request: media_core::BitrateRequest {
                input_path: "source.mkv".into(),
                stream_index: 7,
                window_seconds: 5.0,
            },
            result: media_core::BitrateResult {
                stream_index: 7,
                window_seconds: 5.0,
                packet_bytes: "9007199254740993".into(),
                packet_count: "3".into(),
                untimed_packet_bytes: "1".into(),
                untimed_packet_count: "1".into(),
                dts_fallback_count: "1".into(),
                start_seconds: Some(0.0),
                end_seconds: Some(5.5),
                average_megabits_per_second: Some(1.0),
                peak_window_megabits_per_second: 2.0,
                points: vec![
                    media_core::BitratePoint {
                        start_seconds: 0.0,
                        megabits_per_second: 2.0,
                        packet_bytes: "9007199254740990".into(),
                    },
                    media_core::BitratePoint {
                        start_seconds: 5.0,
                        megabits_per_second: 1.0,
                        packet_bytes: "2".into(),
                    },
                ],
                source_fingerprint: "a".repeat(64),
            },
        };
        validate(&value).unwrap();
        let csv = csv(&value);
        assert!(csv.contains("\"packet_bytes\",\"9007199254740993\""));
        assert!(csv.contains("5,10,1,2\r\n"));
        assert!(csv.contains("\"untimed_packet_count\",\"1\""));
        let svg = svg(&value);
        assert!(svg.contains("Packet bitrate · stream #7"));
        assert!(svg.contains("Mb/s"));
    }
    #[test]
    fn inconsistent_and_nonfinite_snapshots_are_rejected() {
        let mut value = report();
        if let AnalysisReport::Quality { result, .. } = &mut value {
            result.points[0].score = Some(f64::NAN);
        }
        assert!(validate(&value).is_err());
        let mut value = report();
        if let AnalysisReport::Quality { request, .. } = &mut value {
            request.frame_count = 60001;
        }
        assert!(validate(&value).is_err());
    }
}
