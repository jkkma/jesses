use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = PathBuf::from(
        env::args_os()
            .nth(1)
            .ok_or("provide the output TypeScript file path")?,
    );
    if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::write(&output, media_core::typescript_contracts())?;
    println!("Generated {}", output.display());
    Ok(())
}
