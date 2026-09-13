//! Read an explicit QualityRequest JSON file and print its read-only result.
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("Usage: analyze_quality REQUEST.json")?;
    let request = serde_json::from_slice(&std::fs::read(path)?)?;
    let (owner, cancel) = tokio::sync::watch::channel(false);
    let interrupt = tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        let _ = owner.send(true);
        std::future::pending::<()>().await;
    });
    let result = media_runtime::analyze_quality(request, cancel).await;
    interrupt.abort();
    println!("{}", serde_json::to_string_pretty(&result?)?);
    Ok(())
}
