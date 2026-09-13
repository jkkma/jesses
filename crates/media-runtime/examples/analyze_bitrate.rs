//! Read a bounded BitrateRequest JSON file and print its read-only result.
use std::io::Read;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("provide a bitrate request JSON file")?;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(65537)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err("request exceeds 64 KiB".into());
    }
    let request = serde_json::from_slice(&bytes)?;
    let (owner, cancel) = tokio::sync::watch::channel(false);
    let interrupt = tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = owner.send(true);
        std::future::pending::<()>().await;
    });
    let result = media_runtime::analyze_bitrate(request, cancel).await;
    interrupt.abort();
    println!("{}", serde_json::to_string_pretty(&result?)?);
    Ok(())
}
