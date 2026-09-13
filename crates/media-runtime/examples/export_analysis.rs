//! Export a saved analysis snapshot without reopening or modifying its sources.
use std::io::Read;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("provide an export request JSON file")?;
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 16 * 1024 * 1024 {
        return Err("request exceeds 16 MiB".into());
    }
    let request = serde_json::from_slice(&bytes)?;
    println!("{}", media_runtime::jobs::export_analysis(request)?);
    Ok(())
}
