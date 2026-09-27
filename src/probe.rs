//! Media file info via ffprobe.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct MediaInfo {
    pub duration: f64,
    /// Display size (rotation already applied), if the file has video.
    pub video: Option<(u32, u32)>,
    pub has_audio: bool,
}

#[derive(Deserialize)]
struct Probe {
    #[serde(default)]
    streams: Vec<Stream>,
    format: Option<Format>,
}

#[derive(Deserialize)]
struct Stream {
    codec_type: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    #[serde(default)]
    side_data_list: Vec<SideData>,
    #[serde(default)]
    tags: Tags,
    duration: Option<String>,
}

#[derive(Deserialize, Default)]
struct Tags {
    rotate: Option<String>,
}

#[derive(Deserialize)]
struct SideData {
    rotation: Option<f64>,
}

#[derive(Deserialize)]
struct Format {
    duration: Option<String>,
}

pub fn info(path: &Path) -> Result<MediaInfo> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_streams", "-show_format", "-of", "json"])
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
    let p: Probe = serde_json::from_slice(&out.stdout)
        .with_context(|| format!("unexpected ffprobe output for {}", path.display()))?;

    let video_stream = p
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"));
    let video = video_stream.and_then(|s| {
        let (w, h) = (s.width?, s.height?);
        let rot = s
            .side_data_list
            .iter()
            .find_map(|d| d.rotation)
            .or_else(|| s.tags.rotate.as_deref().and_then(|r| r.parse().ok()))
            .unwrap_or(0.0);
        let quarter = ((rot / 90.0).round() as i64).rem_euclid(2) == 1;
        Some(if quarter { (h, w) } else { (w, h) })
    });
    let has_audio = p
        .streams
        .iter()
        .any(|s| s.codec_type.as_deref() == Some("audio"));

    let duration = p
        .format
        .and_then(|f| f.duration)
        .or_else(|| p.streams.iter().find_map(|s| s.duration.clone()))
        .and_then(|d| d.parse::<f64>().ok());
    let duration = match duration {
        Some(d) => d,
        // still images have no duration
        None if video.is_some() => 0.0,
        None => bail!("could not get duration of {}", path.display()),
    };

    Ok(MediaInfo {
        duration,
        video,
        has_audio,
    })
}
