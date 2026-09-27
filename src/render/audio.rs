//! Audio mixing in Rust, so Lua can animate volume and pan per frame.
//! Every source is decoded by ffmpeg to 48 kHz stereo float, scaled by a gain
//! that ramps smoothly from frame to frame, summed, and written to a WAV file
//! the encoder then muxes in.

use crate::check::Span;
use crate::project::LayerType;
use crate::scene::LayerState;
use crate::script::{Env, Script};
use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::{BufReader, BufWriter, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};

pub const RATE: u32 = 48_000;
const CHANNELS: usize = 2;

struct Track {
    span: Span,
    base: LayerState,
    script: Option<Script>,
    decoder: Option<PcmDecoder>,
    /// Gain at the end of the previous frame, per channel.
    prev: Option<(f32, f32)>,
}

/// Mixes all audio of the project in [from, to) into `out`.
/// Returns false (and writes nothing) if no layer has sound.
pub fn mix(spans: &[Span], env: &Env, from: f64, to: f64, frames: u64, out: &Path) -> Result<bool> {
    let mut tracks = Vec::new();
    for s in spans {
        let has_audio = match s.kind {
            LayerType::Audio => true,
            LayerType::Video => s.media.as_ref().is_some_and(|m| m.has_audio),
            _ => false,
        };
        if !has_audio || s.end <= from || s.start >= to {
            continue;
        }
        let script = Script::load(s, env).with_context(|| format!("layer \"{}\"", s.name))?;
        tracks.push(Track {
            span: s.clone(),
            base: s.state.clone().expect("check guarantees a valid state"),
            script,
            decoder: None,
            prev: None,
        });
    }
    if tracks.is_empty() {
        return Ok(false);
    }

    let mut wav = WavWriter::create(out)?;
    let mut mix_buf: Vec<f32> = Vec::new();
    let mut src_buf: Vec<f32> = Vec::new();
    let fps = env.fps;

    for i in 0..frames {
        let t = from + i as f64 / fps;
        let s0 = (i as f64 * RATE as f64 / fps).round() as usize;
        let s1 = ((i + 1) as f64 * RATE as f64 / fps).round() as usize;
        let n = s1 - s0;
        mix_buf.clear();
        mix_buf.resize(n * CHANNELS, 0.0);

        for tr in tracks.iter_mut() {
            const EPS: f64 = 1e-9;
            if !(t + EPS >= tr.span.start && t + EPS < tr.span.end) {
                if t >= tr.span.end {
                    tr.decoder = None;
                }
                continue;
            }
            let mut st = tr.base.clone();
            if let Some(script) = &tr.script {
                script
                    .apply(t - tr.span.start, t, &mut st)
                    .with_context(|| format!("layer \"{}\" at {}", tr.span.name, crate::time::format(t)))?;
            }
            let vol = st.volume.max(0.0) as f32;
            let pan = st.pan.clamp(-1.0, 1.0) as f32;
            let gain = (vol * (1.0 - pan).min(1.0), vol * (1.0 + pan).min(1.0));
            // first frame of a layer starts at its own gain; after that, ramp
            let prev = tr.prev.unwrap_or(gain);
            tr.prev = Some(gain);

            if tr.decoder.is_none() {
                tr.decoder = Some(PcmDecoder::open(
                    tr.span.source.as_ref().unwrap(),
                    tr.span.trim_in + (t - tr.span.start),
                    tr.span.end - t + 1.0,
                )?);
            }
            src_buf.clear();
            src_buf.resize(n * CHANNELS, 0.0);
            tr.decoder.as_mut().unwrap().read(&mut src_buf)?;

            if gain == (0.0, 0.0) && prev == (0.0, 0.0) {
                continue;
            }
            for j in 0..n {
                let k = (j + 1) as f32 / n as f32;
                let gl = prev.0 + (gain.0 - prev.0) * k;
                let gr = prev.1 + (gain.1 - prev.1) * k;
                mix_buf[j * 2] += src_buf[j * 2] * gl;
                mix_buf[j * 2 + 1] += src_buf[j * 2 + 1] * gr;
            }
        }
        wav.write(&mix_buf)?;
    }
    wav.finish()?;
    Ok(true)
}

/// Decodes any audio to interleaved stereo f32 at 48 kHz.
struct PcmDecoder {
    child: Child,
    stdout: BufReader<ChildStdout>,
    bytes: Vec<u8>,
    eof: bool,
}

impl PcmDecoder {
    fn open(path: &Path, seek: f64, duration: f64) -> Result<PcmDecoder> {
        let mut child = Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error"])
            .args(["-ss", &format!("{seek:.6}")])
            .arg("-i")
            .arg(path)
            .args(["-t", &format!("{duration:.6}")])
            .args(["-vn", "-sn", "-ac", "2", "-ar", &RATE.to_string(), "-f", "f32le", "pipe:1"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("failed to run ffmpeg — is it installed?")?;
        let stdout = BufReader::with_capacity(1 << 16, child.stdout.take().unwrap());
        Ok(PcmDecoder { child, stdout, bytes: Vec::new(), eof: false })
    }

    /// Fills `out` with samples; silence once the source ends.
    fn read(&mut self, out: &mut [f32]) -> Result<()> {
        if self.eof {
            return Ok(());
        }
        self.bytes.resize(out.len() * 4, 0);
        let mut filled = 0;
        while filled < self.bytes.len() {
            match self.stdout.read(&mut self.bytes[filled..]) {
                Ok(0) => {
                    self.eof = true;
                    break;
                }
                Ok(n) => filled += n,
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(e).context("reading decoded audio"),
            }
        }
        for (o, b) in out.iter_mut().zip(self.bytes[..filled - filled % 4].chunks_exact(4)) {
            *o = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        }
        Ok(())
    }
}

impl Drop for PcmDecoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Minimal 32-bit float WAV writer.
struct WavWriter {
    file: BufWriter<File>,
    data_bytes: u64,
}

impl WavWriter {
    fn create(path: &Path) -> Result<WavWriter> {
        let file = File::create(path).with_context(|| format!("can't create {}", path.display()))?;
        let mut w = WavWriter { file: BufWriter::new(file), data_bytes: 0 };
        w.header(0)?;
        Ok(w)
    }

    fn header(&mut self, data: u32) -> Result<()> {
        let f = &mut self.file;
        let block = (CHANNELS * 4) as u16;
        f.write_all(b"RIFF")?;
        f.write_all(&(36 + data).to_le_bytes())?;
        f.write_all(b"WAVEfmt ")?;
        f.write_all(&16u32.to_le_bytes())?;
        f.write_all(&3u16.to_le_bytes())?; // IEEE float
        f.write_all(&(CHANNELS as u16).to_le_bytes())?;
        f.write_all(&RATE.to_le_bytes())?;
        f.write_all(&(RATE * block as u32).to_le_bytes())?;
        f.write_all(&block.to_le_bytes())?;
        f.write_all(&32u16.to_le_bytes())?;
        f.write_all(b"data")?;
        f.write_all(&data.to_le_bytes())?;
        Ok(())
    }

    fn write(&mut self, samples: &[f32]) -> Result<()> {
        for s in samples {
            self.file.write_all(&s.to_le_bytes())?;
        }
        self.data_bytes += samples.len() as u64 * 4;
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        if self.data_bytes > u32::MAX as u64 - 36 {
            bail!("audio is too long for a WAV file (over ~6 hours)");
        }
        self.file.seek(SeekFrom::Start(0))?;
        self.header(self.data_bytes as u32)?;
        self.file.flush()?;
        Ok(())
    }
}

/// Temporary file removed when dropped, even if the render fails.
pub struct TempFile(pub PathBuf);

impl Drop for TempFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
