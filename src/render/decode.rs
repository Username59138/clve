//! Video decoding: one ffmpeg process per video layer, streaming raw RGBA
//! frames already resampled to the project frame rate and target size.

use anyhow::{bail, Context, Result};
use std::io::{BufReader, ErrorKind, Read};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::thread::JoinHandle;
use tiny_skia::{IntSize, Pixmap};

pub struct VideoDecoder {
    child: Child,
    stdout: BufReader<ChildStdout>,
    stderr: Option<JoinHandle<String>>,
    buf: Vec<u8>,
    frame: Pixmap,
    has_frame: bool,
    eof: bool,
}

impl VideoDecoder {
    /// Starts decoding `path` from `seek` seconds for `duration` seconds.
    pub fn open(path: &Path, seek: f64, duration: f64, fps: f64, w: u32, h: u32) -> Result<Self> {
        let filters = format!("fps={},scale={w}:{h}:flags=bicubic,setsar=1,format=rgba", super::encode::fps_arg(fps));
        let mut child = Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error"])
            .args(["-ss", &format!("{seek:.6}")])
            .arg("-i")
            .arg(path)
            .args(["-t", &format!("{duration:.6}")])
            .args(["-an", "-sn", "-vf", &filters, "-f", "rawvideo", "pipe:1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("failed to run ffmpeg — is it installed?")?;
        let stdout = BufReader::with_capacity(1 << 20, child.stdout.take().unwrap());
        let mut err = child.stderr.take().unwrap();
        let stderr = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = err.read_to_string(&mut s);
            s
        });
        let frame = Pixmap::new(w, h).context("invalid video frame size")?;
        Ok(VideoDecoder {
            child,
            stdout,
            stderr: Some(stderr),
            buf: vec![0; (w * h * 4) as usize],
            frame,
            has_frame: false,
            eof: false,
        })
    }

    /// Next frame. When the source runs out, the last frame is held.
    pub fn next(&mut self) -> Result<&Pixmap> {
        if !self.eof {
            // read into a side buffer so a truncated last frame never shows up
            match self.stdout.read_exact(&mut self.buf) {
                Ok(()) => {
                    let data = self.frame.data_mut();
                    data.copy_from_slice(&self.buf);
                    premultiply(data);
                    self.has_frame = true;
                }
                Err(e) if e.kind() == ErrorKind::UnexpectedEof => {
                    self.eof = true;
                    let status = self.child.wait()?;
                    let log = self.stderr.take().and_then(|h| h.join().ok()).unwrap_or_default();
                    if !status.success() {
                        bail!("ffmpeg failed to decode: {}", log.trim());
                    }
                    if !self.has_frame {
                        bail!("ffmpeg produced no frames{}", suffix(&log));
                    }
                }
                Err(e) => return Err(e).context("reading decoded video"),
            }
        }
        Ok(&self.frame)
    }
}

impl Drop for VideoDecoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn suffix(log: &str) -> String {
    let log = log.trim();
    if log.is_empty() {
        String::new()
    } else {
        format!(": {log}")
    }
}

/// ffmpeg gives straight alpha, tiny-skia wants premultiplied.
pub fn premultiply(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u16;
        if a != 255 {
            px[0] = ((px[0] as u16 * a + 127) / 255) as u8;
            px[1] = ((px[1] as u16 * a + 127) / 255) as u8;
            px[2] = ((px[2] as u16 * a + 127) / 255) as u8;
        }
    }
}

/// Loads an image file into a premultiplied pixmap.
pub fn load_image(path: &Path) -> Result<Pixmap> {
    let img = image::open(path)
        .with_context(|| format!("can't read image {}", path.display()))?
        .into_rgba8();
    let (w, h) = img.dimensions();
    let mut data = img.into_raw();
    premultiply(&mut data);
    Pixmap::from_vec(data, IntSize::from_wh(w, h).context("empty image")?)
        .context("invalid image size")
}
