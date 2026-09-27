//! Encoding: raw frames go into ffmpeg's stdin, the pre-mixed audio comes from a WAV file.

use crate::project::RenderConfig;
use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::thread::JoinHandle;

pub struct Encoder {
    child: Child,
    stdin: Option<ChildStdin>,
    stderr: Option<JoinHandle<String>>,
}

pub struct EncodeSettings {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: f64,
    pub preview: bool,
}

/// Output container, picked from the file extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// mp4, mov, mkv: H.264/H.265 + AAC
    Video,
    /// VP9 + Opus
    Webm,
    /// Animated GIF, no sound
    Gif,
}

impl Format {
    pub fn from_path(p: &Path) -> Result<Format> {
        let ext = p
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        Ok(match ext.as_str() {
            "mp4" | "mov" | "mkv" | "m4v" => Format::Video,
            "webm" => Format::Webm,
            "gif" => Format::Gif,
            "" => bail!("output {} has no extension (use .mp4, .mov, .mkv, .webm or .gif)", p.display()),
            other => bail!("can't write .{other} files (use .mp4, .mov, .mkv, .webm or .gif)"),
        })
    }

    pub fn has_audio(self) -> bool {
        self != Format::Gif
    }
}

impl Encoder {
    pub fn start(out: &Path, cfg: &RenderConfig, set: &EncodeSettings, audio: Option<&Path>) -> Result<Encoder> {
        let format = Format::from_path(out)?;
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-nostdin", "-y", "-v", "error"]);
        cmd.args(["-f", "rawvideo", "-pix_fmt", "rgba"])
            .args(["-s", &format!("{}x{}", set.width, set.height)])
            .args(["-framerate", &fps_arg(set.fps)])
            .args(["-i", "pipe:0"]);
        let audio = audio.filter(|_| format.has_audio());
        if let Some(wav) = audio {
            cmd.arg("-i").arg(wav);
        }

        let quality = if set.preview { "preview" } else { cfg.quality.as_str() };
        match format {
            Format::Video => {
                cmd.args(["-map", "0:v"]);
                if audio.is_some() {
                    cmd.args(["-map", "1:a", "-c:a", "aac", "-b:a", "192k"]);
                }
                let (crf, preset) = match quality {
                    "preview" => (30, "ultrafast"),
                    "low" => (28, "veryfast"),
                    "medium" => (23, "medium"),
                    "lossless" => (0, "medium"),
                    _ => (18, "slow"),
                };
                let crf = if set.preview { crf } else { cfg.crf.unwrap_or(crf) };
                if cfg.codec == "h265" {
                    let params = if crf == 0 { "log-level=error:lossless=1" } else { "log-level=error" };
                    cmd.args(["-c:v", "libx265", "-tag:v", "hvc1", "-x265-params", params]);
                } else {
                    cmd.args(["-c:v", "libx264"]);
                }
                cmd.args(["-preset", preset, "-crf", &crf.to_string()])
                    .args(["-pix_fmt", "yuv420p", "-movflags", "+faststart"]);
            }
            Format::Webm => {
                cmd.args(["-map", "0:v"]);
                if audio.is_some() {
                    cmd.args(["-map", "1:a", "-c:a", "libopus", "-b:a", "160k"]);
                }
                let (crf, deadline, cpu) = match quality {
                    "preview" => (40, "realtime", "8"),
                    "low" => (38, "good", "5"),
                    "medium" => (31, "good", "4"),
                    "lossless" => (0, "good", "4"),
                    _ => (24, "good", "2"),
                };
                let crf = if set.preview { crf } else { cfg.crf.map(|c| c.min(63)).unwrap_or(crf) };
                cmd.args(["-c:v", "libvpx-vp9", "-b:v", "0", "-crf", &crf.to_string()])
                    .args(["-deadline", deadline, "-cpu-used", cpu, "-row-mt", "1"])
                    .args(["-pix_fmt", "yuv420p"]);
                if crf == 0 {
                    cmd.args(["-lossless", "1"]);
                }
            }
            Format::Gif => {
                // one palette for the whole clip, only changed pixels redrawn: small and clean
                cmd.args([
                    "-filter_complex",
                    "[0:v]split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=sierra2_4a:diff_mode=rectangle",
                    "-loop",
                    "0",
                ]);
            }
        }
        cmd.args(["-t", &format!("{:.6}", set.duration)]).arg(out);

        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to run ffmpeg — is it installed?")?;
        let stdin = child.stdin.take();
        let mut err = child.stderr.take().unwrap();
        let stderr = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = err.read_to_string(&mut s);
            s
        });
        Ok(Encoder {
            child,
            stdin,
            stderr: Some(stderr),
        })
    }

    pub fn write(&mut self, rgba: &[u8]) -> Result<()> {
        let stdin = self.stdin.as_mut().unwrap();
        if stdin.write_all(rgba).is_err() {
            // ffmpeg quit early; its log explains why
            return self.finish().and_then(|_| bail!("ffmpeg stopped accepting frames"));
        }
        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        drop(self.stdin.take());
        let status = self.child.wait()?;
        let log = self.stderr.take().and_then(|h| h.join().ok()).unwrap_or_default();
        if !status.success() {
            bail!("ffmpeg failed to encode: {}", log.trim());
        }
        Ok(())
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        if self.stdin.is_some() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// 29.97 and friends go in as exact NTSC fractions.
pub fn fps_arg(fps: f64) -> String {
    for (num, den) in [(24000, 1001), (30000, 1001), (60000, 1001)] {
        if (fps - num as f64 / den as f64).abs() < 0.005 {
            return format!("{num}/{den}");
        }
    }
    format!("{fps}")
}
