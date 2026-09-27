//! Encoding: raw frames go into ffmpeg's stdin; audio from every layer is
//! trimmed, delayed and mixed by ffmpeg in the same process.

use crate::check::Span;
use crate::project::{LayerType, RenderConfig};
use anyhow::{bail, Context, Result};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
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
    pub from: f64,
    pub to: f64,
    pub preview: bool,
}

/// An audio source cut to the render range.
struct AudioClip {
    path: PathBuf,
    seek: f64,
    duration: f64,
    delay: f64,
    volume: f64,
}

fn audio_clips(spans: &[Span], from: f64, to: f64) -> Vec<AudioClip> {
    let mut clips = Vec::new();
    for s in spans {
        let has_audio = match s.kind {
            LayerType::Audio => true,
            LayerType::Video => s.media.as_ref().is_some_and(|m| m.has_audio),
            _ => false,
        };
        let volume = s.state.as_ref().map_or(1.0, |st| st.volume);
        let (Some(path), true) = (&s.source, has_audio) else { continue };
        let begin = s.start.max(from);
        let end = s.end.min(to);
        if end <= begin || volume <= 0.0 {
            continue;
        }
        clips.push(AudioClip {
            path: path.clone(),
            seek: s.trim_in + (begin - s.start),
            duration: end - begin,
            delay: begin - from,
            volume,
        });
    }
    clips
}

impl Encoder {
    pub fn start(out: &Path, cfg: &RenderConfig, set: &EncodeSettings, spans: &[Span]) -> Result<Encoder> {
        let duration = set.to - set.from;
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-nostdin", "-y", "-v", "error"]);
        cmd.args(["-f", "rawvideo", "-pix_fmt", "rgba"])
            .args(["-s", &format!("{}x{}", set.width, set.height)])
            .args(["-framerate", &fps_arg(set.fps)])
            .args(["-i", "pipe:0"]);

        let clips = audio_clips(spans, set.from, set.to);
        for c in &clips {
            cmd.args(["-ss", &format!("{:.6}", c.seek)])
                .args(["-t", &format!("{:.6}", c.duration)])
                .arg("-i")
                .arg(&c.path);
        }
        cmd.args(["-map", "0:v"]);
        if !clips.is_empty() {
            let mut graph = String::new();
            for (i, c) in clips.iter().enumerate() {
                let ms = (c.delay * 1000.0).round() as u64;
                graph += &format!(
                    "[{}:a:0]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo,volume={:.4},adelay={ms}:all=1[a{i}];",
                    i + 1,
                    c.volume
                );
            }
            for i in 0..clips.len() {
                graph += &format!("[a{i}]");
            }
            graph += &format!(
                "amix=inputs={}:normalize=0:dropout_transition=0[aout]",
                clips.len()
            );
            cmd.args(["-filter_complex", &graph, "-map", "[aout]"]);
            cmd.args(["-c:a", "aac", "-b:a", "192k"]);
        }

        let (crf, preset) = if set.preview {
            (30, "ultrafast")
        } else {
            match cfg.quality.as_str() {
                "low" => (28, "veryfast"),
                "medium" => (23, "medium"),
                "lossless" => (0, "medium"),
                _ => (18, "slow"),
            }
        };
        let crf = if set.preview { crf } else { cfg.crf.unwrap_or(crf) };
        match cfg.codec.as_str() {
            "h265" => {
                let params = if crf == 0 { "log-level=error:lossless=1" } else { "log-level=error" };
                cmd.args(["-c:v", "libx265", "-tag:v", "hvc1", "-x265-params", params]);
            }
            _ => {
                cmd.args(["-c:v", "libx264"]);
            }
        }
        cmd.args(["-preset", preset, "-crf", &crf.to_string()])
            .args(["-pix_fmt", "yuv420p", "-movflags", "+faststart"])
            .args(["-t", &format!("{duration:.6}")])
            .arg(out);

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
