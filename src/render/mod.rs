//! clve render: composite every frame in Rust, encode with ffmpeg.

mod decode;
mod encode;
mod text;

use crate::check::{self, Span};
use crate::project::{Fit, LayerType, Project};
use crate::scene::LayerState;
use crate::time;
use anyhow::{bail, Context, Result};
use decode::VideoDecoder;
use encode::{EncodeSettings, Encoder};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::Instant;
use text::{TextKey, TextRenderer};
use tiny_skia::{
    Color, FilterQuality, Paint, Pixmap, PixmapPaint, Rect, Transform,
};

pub struct Options {
    pub output: Option<PathBuf>,
    pub from: Option<f64>,
    pub to: Option<f64>,
    pub preview: bool,
    /// Render a single frame at this time into a PNG.
    pub frame: Option<f64>,
}

const PREVIEW_HEIGHT: u32 = 480;

pub fn run(project: &Project, opts: Options) -> Result<()> {
    let report = check::analyze(project);
    for w in &report.warnings {
        eprintln!("warning: {}{}", layer_prefix(&w.layer), w.message);
    }
    if !report.errors.is_empty() {
        for e in &report.errors {
            eprintln!("error: {}{}", layer_prefix(&e.layer), e.message);
        }
        bail!("fix the errors above before rendering (clve check)");
    }
    let total = report.duration;
    if total <= 0.0 {
        bail!("nothing to render: the project has no layers");
    }

    let cfg = &project.file.render;
    let fps = cfg.fps;
    let [pw, ph] = cfg.resolution;

    // time range
    let (from, to, frames) = if let Some(t) = opts.frame {
        if t < 0.0 || t >= total {
            bail!("--frame {}: outside the project (length {})", time::format(t), time::format(total));
        }
        (t, t + 1.0 / fps, 1u64)
    } else {
        let from = opts.from.unwrap_or(0.0);
        let to = opts.to.unwrap_or(total).min(total);
        if from >= to {
            bail!("empty range: --from {} --to {}", time::format(from), time::format(to));
        }
        let frames = ((to - from) * fps).round().max(1.0) as u64;
        (from, to, frames)
    };

    // output size: preview renders small, everything else at full resolution
    let k = if opts.preview && ph > PREVIEW_HEIGHT {
        PREVIEW_HEIGHT as f64 / ph as f64
    } else {
        1.0
    };
    let (ow, oh) = (even(pw as f64 * k), even(ph as f64 * k));

    let out = output_path(project, &opts)?;
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("can't create {}", dir.display()))?;
    }

    let mut text = TextRenderer::new();
    // visual layers, bottom to top
    let mut visuals: Vec<Runtime> = report
        .layers
        .iter()
        .filter(|s| !s.kind.is_audio_only())
        .map(|s| Runtime::new(s.clone()))
        .collect();
    visuals.sort_by_key(|r| r.span.z);
    for r in &visuals {
        if r.span.kind == LayerType::Text && !text.has_family(&r.base.font) {
            eprintln!(
                "warning: [{}] font \"{}\" is not installed, using a fallback",
                r.span.name, r.base.font
            );
        }
    }

    let ctx = Ctx { fps, k, canvas: (pw as f64, ph as f64) };
    let mut canvas = Pixmap::new(ow, oh).context("invalid output size")?;

    let mut encoder = if opts.frame.is_none() {
        eprintln!(
            "rendering {} → {} ({ow}x{oh}, {} fps, {})",
            project.file.project.name,
            display_path(project, &out),
            fps,
            time::format(to - from)
        );
        let set = EncodeSettings { width: ow, height: oh, fps, from, to, preview: opts.preview };
        Some(Encoder::start(&out, cfg, &set, &report.layers)?)
    } else {
        None
    };

    let mut progress = Progress::new(frames);
    for i in 0..frames {
        let t = from + i as f64 / fps;
        canvas.fill(Color::BLACK);
        for layer in visuals.iter_mut() {
            layer
                .draw(&mut canvas, t, &ctx, &mut text)
                .with_context(|| format!("layer \"{}\"", layer.span.name))?;
        }
        if let Some(enc) = encoder.as_mut() {
            enc.write(canvas.data())?;
            progress.tick(i + 1);
        }
    }

    match encoder.as_mut() {
        Some(enc) => {
            enc.finish()?;
            progress.done(&display_path(project, &out));
        }
        None => {
            canvas
                .save_png(&out)
                .with_context(|| format!("can't write {}", out.display()))?;
            eprintln!("frame {} → {}", time::format(from), display_path(project, &out));
        }
    }
    Ok(())
}

struct Ctx {
    fps: f64,
    /// Output pixels per project pixel.
    k: f64,
    canvas: (f64, f64),
}

/// A visual layer while the render runs.
struct Runtime {
    span: Span,
    base: LayerState,
    video: Option<VideoDecoder>,
    image: Option<Pixmap>,
    text: Option<(TextKey, Option<Pixmap>, f64)>,
}

impl Runtime {
    fn new(span: Span) -> Runtime {
        let base = span.state.clone().expect("check guarantees a valid state");
        Runtime { span, base, video: None, image: None, text: None }
    }

    fn draw(&mut self, canvas: &mut Pixmap, t: f64, ctx: &Ctx, text: &mut TextRenderer) -> Result<()> {
        const EPS: f64 = 1e-9;
        let active = t + EPS >= self.span.start && t + EPS < self.span.end;
        if !active {
            // free the decoder as soon as the layer is over
            if t >= self.span.end {
                self.video = None;
                self.image = None;
            }
            return Ok(());
        }

        // Per-frame state. This is where Lua will change things.
        let st = self.base.clone();
        if !st.visible || st.opacity <= 0.0 {
            // still advance the video so it stays in sync
            if self.span.kind == LayerType::Video {
                self.video_frame(t, ctx)?;
            }
            return Ok(());
        }

        let (w, h) = ctx.canvas;
        match self.span.kind {
            LayerType::Color => {
                let [r, g, b, a] = st.color;
                let mut paint = Paint::default();
                paint.set_color_rgba8(r, g, b, (a as f64 * st.opacity).round() as u8);
                paint.anti_alias = true;
                let tr = layer_transform(&st, ctx.k, w, h);
                let rect = Rect::from_xywh(0.0, 0.0, w as f32, h as f32).unwrap();
                canvas.fill_rect(rect, &paint, tr, None);
            }
            LayerType::Video => {
                let (cw, ch) = self.fitted_size(&st, ctx)?;
                let frame = self.video_frame(t, ctx)?;
                let (fw, fh) = (frame.width() as f64, frame.height() as f64);
                let tr = layer_transform(&st, ctx.k, cw, ch).pre_scale((cw / fw) as f32, (ch / fh) as f32);
                draw_pixmap(canvas, frame, tr, st.opacity);
            }
            LayerType::Image => {
                if self.image.is_none() {
                    self.image = Some(decode::load_image(self.span.source.as_ref().unwrap())?);
                }
                let (cw, ch) = self.fitted_size(&st, ctx)?;
                let img = self.image.as_ref().unwrap();
                let (iw, ih) = (img.width() as f64, img.height() as f64);
                let tr = layer_transform(&st, ctx.k, cw, ch).pre_scale((cw / iw) as f32, (ch / ih) as f32);
                draw_pixmap(canvas, img, tr, st.opacity);
            }
            LayerType::Text => {
                // rasterize at the size it will be shown, so scaled-up text stays sharp
                let raster = ctx.k * quantize(st.scale.max(1.0));
                let key = TextKey::new(&st, (st.size * raster) as f32);
                let stale = self.text.as_ref().is_none_or(|(k, _, _)| *k != key);
                if stale {
                    let pix = text.render(&key);
                    self.text = Some((key, pix, raster));
                }
                let (_, pix, raster) = self.text.as_ref().unwrap();
                if let Some(pix) = pix {
                    let pad = (st.size * raster * 0.25).ceil();
                    let pw = pix.width() as f64;
                    let ph = pix.height() as f64;
                    let cw = (pw - pad * 2.0) / raster;
                    let ch = (ph - pad * 2.0) / raster;
                    let tr = layer_transform(&st, ctx.k, cw, ch)
                        .pre_scale((1.0 / raster) as f32, (1.0 / raster) as f32)
                        .pre_translate(-pad as f32, -pad as f32);
                    draw_pixmap(canvas, pix, tr, st.opacity);
                }
            }
            LayerType::Audio => {}
        }
        Ok(())
    }

    /// Size of a video/image on the canvas in project pixels, before `scale`.
    fn fitted_size(&self, st: &LayerState, ctx: &Ctx) -> Result<(f64, f64)> {
        let (vw, vh) = match self.span.kind {
            LayerType::Video => self
                .span
                .media
                .as_ref()
                .and_then(|m| m.video)
                .context("no video stream")?,
            _ => self.span.image_size.context("unknown image size")?,
        };
        let (vw, vh) = (vw as f64, vh as f64);
        let (w, h) = ctx.canvas;
        Ok(match st.fit {
            Fit::Contain => {
                let s = (w / vw).min(h / vh);
                (vw * s, vh * s)
            }
            Fit::Cover => {
                let s = (w / vw).max(h / vh);
                (vw * s, vh * s)
            }
            Fit::Stretch => (w, h),
            Fit::None => (vw, vh),
        })
    }

    fn video_frame(&mut self, t: f64, ctx: &Ctx) -> Result<&Pixmap> {
        if self.video.is_none() {
            let (cw, ch) = self.fitted_size(&self.base, ctx)?;
            // decode at the size it will be shown (never bigger than 2x the output)
            let s = ctx.k * quantize(self.base.scale.max(1.0)).min(2.0);
            let (dw, dh) = (even(cw * s), even(ch * s));
            let offset = t - self.span.start;
            self.video = Some(VideoDecoder::open(
                self.span.source.as_ref().unwrap(),
                self.span.trim_in + offset,
                self.span.end - t + 1.0,
                ctx.fps,
                dw,
                dh,
            )?);
        }
        self.video.as_mut().unwrap().next()
    }
}

/// Maps layer content (0..cw, 0..ch in project pixels) to output pixels:
/// move the anchor to (x, y), rotate and scale around it.
fn layer_transform(st: &LayerState, k: f64, cw: f64, ch: f64) -> Transform {
    Transform::from_scale(k as f32, k as f32)
        .pre_translate(st.x as f32, st.y as f32)
        .pre_rotate(st.rotation as f32)
        .pre_scale(st.scale as f32, st.scale as f32)
        .pre_translate((-st.anchor.0 * cw) as f32, (-st.anchor.1 * ch) as f32)
}

fn draw_pixmap(canvas: &mut Pixmap, pix: &Pixmap, tr: Transform, opacity: f64) {
    // pixel-aligned copies don't need filtering
    let aligned = tr.sx == 1.0
        && tr.sy == 1.0
        && tr.kx == 0.0
        && tr.ky == 0.0
        && tr.tx.fract() == 0.0
        && tr.ty.fract() == 0.0;
    let paint = PixmapPaint {
        opacity: opacity.clamp(0.0, 1.0) as f32,
        quality: if aligned { FilterQuality::Nearest } else { FilterQuality::Bilinear },
        ..Default::default()
    };
    canvas.draw_pixmap(0, 0, pix.as_ref(), &paint, tr, None);
}

/// Rounds a scale up to a quarter step so small changes don't force a re-render.
fn quantize(s: f64) -> f64 {
    (s * 4.0).ceil() / 4.0
}

fn even(v: f64) -> u32 {
    ((v / 2.0).round() as u32 * 2).max(2)
}

fn output_path(project: &Project, opts: &Options) -> Result<PathBuf> {
    if let Some(o) = &opts.output {
        return Ok(o.clone());
    }
    let name = &project.file.project.name;
    let dir = project.root.join(&project.file.render.output);
    Ok(match opts.frame {
        Some(t) => dir.join(format!("{name}-frame-{:.2}.png", t)),
        None if opts.preview => dir.join(format!("{name}-preview.mp4")),
        None => dir.join(format!("{name}.mp4")),
    })
}

fn display_path(project: &Project, p: &std::path::Path) -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    p.strip_prefix(&cwd)
        .or_else(|_| p.strip_prefix(&project.root))
        .unwrap_or(p)
        .display()
        .to_string()
}

fn layer_prefix(layer: &Option<String>) -> String {
    layer.as_ref().map(|l| format!("[{l}] ")).unwrap_or_default()
}

struct Progress {
    total: u64,
    started: Instant,
    last: Option<Instant>,
    tty: bool,
}

impl Progress {
    fn new(total: u64) -> Progress {
        Progress { total, started: Instant::now(), last: None, tty: std::io::stderr().is_terminal() }
    }

    fn tick(&mut self, done: u64) {
        if !self.tty {
            return;
        }
        let now = Instant::now();
        if self.last.is_some_and(|l| now.duration_since(l).as_millis() < 100) && done < self.total {
            return;
        }
        self.last = Some(now);
        let frac = done as f64 / self.total as f64;
        let bar_w = 24;
        let filled = (frac * bar_w as f64).round() as usize;
        let elapsed = self.started.elapsed().as_secs_f64();
        let eta = if done > 0 { elapsed / done as f64 * (self.total - done) as f64 } else { 0.0 };
        eprint!(
            "\r\x1b[Krendering  {}{}  {:>3}%  frame {done}/{}  eta {}",
            "█".repeat(filled),
            "░".repeat(bar_w - filled),
            (frac * 100.0).round(),
            self.total,
            human(eta)
        );
    }

    fn done(&self, out: &str) {
        if self.tty {
            eprint!("\r\x1b[K");
        }
        let secs = self.started.elapsed().as_secs_f64();
        eprintln!(
            "done: {out} ({} frames in {}, {:.0} fps)",
            self.total,
            human(secs),
            self.total as f64 / secs.max(1e-3)
        );
    }
}

fn human(secs: f64) -> String {
    let s = secs.round() as u64;
    if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}
