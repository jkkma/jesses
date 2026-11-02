//! Actual pinned-provider presets, including every source film-stock modifier.
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use tokio::sync::watch;

fn configure_bundle() {
    static CONFIGURED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    CONFIGURED.get_or_init(|| {
        if let Some(resources) = std::env::var_os("JESSES_GRAIN_RESOURCES") {
            let root = std::path::PathBuf::from(resources).canonicalize().unwrap();
            media_runtime::configure_bundled_tools(root).unwrap();
            assert!(
                std::env::var_os("JESSES_GRAV1SYNTH").is_none(),
                "the bundle gate must not use an explicit provider override"
            );
        }
    });
}

fn parameter_digest(table: &str, expected: &serde_json::Value) -> String {
    let mut segments = Vec::<(u64, u64, String)>::new();
    for line in table.lines() {
        let fields = line.split_whitespace().collect::<Vec<_>>();
        match fields.first().copied() {
            None | Some("filmgrn1") => (),
            Some("E") => segments.push((
                fields[1].parse().unwrap(),
                fields[2].parse().unwrap(),
                String::new(),
            )),
            _ => {
                let parameters = &mut segments.last_mut().expect("preceding segment").2;
                parameters.push_str(&fields.join(" "));
                parameters.push('\n');
            }
        }
    }
    assert_eq!(segments.first().unwrap().0, 0);
    assert!(segments.last().unwrap().1 >= 864_000_000_000);
    let hash = |text: &str| format!("{:x}", Sha256::digest(text.as_bytes()));
    assert_eq!(
        hash(&segments.first().unwrap().2),
        expected["firstParametersSha256"]
    );
    assert_eq!(
        hash(&segments.last().unwrap().2),
        expected["tailParametersSha256"]
    );
    for (_, _, parameters) in &segments {
        assert!(
            expected["parameterStatesSha256"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value.as_str() == Some(&hash(parameters))),
            "every actual parameter state comes from this exact embedded source stock"
        );
    }
    let mut digest = Sha256::new();
    for frame in 0..360u64 {
        let tick = (frame * 10_000_000).div_ceil(24);
        let segment = segments
            .iter()
            .find(|(start, end, _)| *start <= tick && tick < *end)
            .expect("complete15secondcoverage");
        digest.update(segment.2.as_bytes());
    }
    format!("{:x}", digest.finalize())
}

#[tokio::test]
#[ignore = "requires the source-pinned grav1synth provider, FFmpeg and standalone SVT-AV1"]
async fn all_fourteen_source_film_stocks_preserve_embedded_parameter_sequences() {
    configure_bundle();
    if let Some(resources) = std::env::var_os("JESSES_GRAIN_RESOURCES") {
        let root = std::path::PathBuf::from(resources).canonicalize().unwrap();
        let capabilities = media_runtime::get_capabilities().await;
        let provider = capabilities
            .iter()
            .find(|tool| tool.id == "grav1synth")
            .unwrap();
        assert!(provider.available, "{provider:?}");
        assert!(std::path::Path::new(provider.path.as_deref().unwrap()).starts_with(root));
    }
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/grav1synth-presets.json")).unwrap();
    let (_owner, cancel) = watch::channel(false);
    let mut results = BTreeSet::new();
    for preset in expected["presets"].as_array().unwrap() {
        let name = preset["name"].as_str().unwrap();
        let table = media_runtime::make_av1an_grain_preset(name.into(), cancel.clone())
            .await
            .unwrap_or_else(|cause| panic!("{name}: {cause:#?}"));
        if let Some(root) = std::env::var_os("JESSES_GRAIN_EVIDENCE") {
            std::fs::write(
                std::path::PathBuf::from(root).join(format!("{name}.tbl")),
                &table,
            )
            .unwrap();
        }
        let digest = parameter_digest(&table, preset);
        assert!(
            results.insert(digest),
            "each source stock must carry its own parameters"
        );
        eprintln!("PASS source film stock {name}");
    }
    assert_eq!(results.len(), 14);
    assert!(
        media_runtime::make_av1an_grain_preset("Super8-3".into(), cancel.clone())
            .await
            .is_err()
    );
    assert!(
        media_runtime::make_av1an_grain_preset("16mm-4".into(), cancel)
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires the source-pinned grav1synth provider and FFmpeg/FFprobe"]
async fn packaged_provider_measures_padded_decoder_strides_at_eight_and_ten_bits() {
    use media_core::{GrainRequest, UtilityRequest, UtilityResult};
    use media_runtime::supervisor::{self, CommandSpec};
    use std::{ffi::OsString, path::PathBuf, time::Duration};
    configure_bundle();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "jesses-grain-strides-{}-{nonce}",
        std::process::id()
    ));
    std::fs::create_dir(&root).unwrap();
    let (_owner, cancel) = watch::channel(false);
    let ffmpeg =
        std::env::var_os("JESSES_FFMPEG").map_or_else(|| PathBuf::from("ffmpeg"), PathBuf::from);
    for format in ["yuv420p", "yuv420p10le"] {
        let input = root.join(format!("{format}.mkv"));
        let output = root.join(format!("{format}.tbl"));
        let reference = root.join(format!("{format}-denoised.mkv"));
        let mut args: Vec<OsString> = [
            "-v",
            "error",
            "-nostdin",
            "-n",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=24:duration=0.5,noise=alls=8:allf=t+u",
            "-vf",
            &format!("format={format}"),
            "-c:v",
            "ffv1",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        args.push(input.as_os_str().to_owned());
        let result = supervisor::run_capture(
            &CommandSpec {
                executable: ffmpeg.clone(),
                args,
                cwd: None,
            },
            cancel.clone(),
            1024 * 1024,
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let original = std::fs::read(&input).unwrap();
        let result = media_runtime::run_utility(
            UtilityRequest::Grain(GrainRequest::MeasureSource {
                source_path: input.to_string_lossy().into_owned(),
                output_table_path: output.to_string_lossy().into_owned(),
                denoise_strength: 4,
                denoised_output_path: Some(reference.to_string_lossy().into_owned()),
            }),
            cancel.clone(),
        )
        .await
        .unwrap_or_else(|cause| panic!("{format}: {cause:#?}; fixture {}", root.display()));
        let UtilityResult::GrainTable(table) = result else {
            panic!("measured table");
        };
        assert!(table.segment_count > 0);
        assert!(
            media_runtime::read_av1an_grain_table(output.to_string_lossy().into_owned())
                .unwrap()
                .starts_with("filmgrn1\n")
        );
        assert!(reference.is_file());
        assert_eq!(std::fs::read(input).unwrap(), original);
        eprintln!("PASS source measurement 320x240 padded decoder stride {format}");
    }
    std::fs::remove_dir_all(root).unwrap();
}
