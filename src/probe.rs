//! Media file durations via ffprobe.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

pub fn duration(path: &Path) -> Result<f64> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .context("failed to run ffprobe — is ffmpeg installed?")?;
    if !out.status.success() {
        bail!(
            "ffprobe could not read {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let s = String::from_utf8_lossy(&out.stdout);
    s.trim()
        .parse::<f64>()
        .with_context(|| format!("could not get duration of {}", path.display()))
}
