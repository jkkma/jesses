//! Read-only qualification entry point: INPUT AUDIO_STREAM [preserve|mono|stereo].
use media_core::{AudioChannels, LoudnessRequest};
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(2..=3).contains(&args.len()) {
        return Err("Usage: measure_loudness INPUT AUDIO_STREAM [preserve|mono|stereo]".into());
    }
    let channels = match args.get(2).map(String::as_str) {
        None | Some("preserve") => AudioChannels::Preserve,
        Some("mono") => AudioChannels::Mono,
        Some("stereo") => AudioChannels::Stereo,
        _ => return Err("Unknown channel setting".into()),
    };
    let (owner, cancel) = tokio::sync::watch::channel(false);
    let task = tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = owner.send(true);
        std::future::pending::<()>().await;
    });
    let result = media_runtime::measure_loudness(
        LoudnessRequest {
            input_path: args[0].clone(),
            stream_index: args[1].parse()?,
            channels,
            target_lufs: -23.0,
            peak_limit_dbfs: -1.0,
        },
        cancel,
    )
    .await;
    task.abort();
    println!("{}", serde_json::to_string_pretty(&result?)?);
    Ok(())
}
