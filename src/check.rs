//! Resolving layers into time spans and validating the project.
//! The render uses the same resolved spans, so check and render never disagree.

use crate::probe::{self, MediaInfo};
use crate::project::{Layer, LayerType, Project};
use crate::scene::LayerState;
use crate::script::{Env, Script};
use crate::time;
use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize)]
pub struct Span {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: LayerType,
    pub z: i32,
    pub start: f64,
    pub end: f64,
    /// Offset into the source file, seconds.
    #[serde(rename = "in")]
    pub trim_in: f64,
    /// Where the source stops, seconds; after it the last frame is held.
    #[serde(rename = "out", skip_serializing_if = "Option::is_none")]
    pub trim_out: Option<f64>,
    /// Seconds the first frame is held (silent) before the source plays.
    #[serde(skip_serializing_if = "is_zero")]
    pub hold: f64,
    #[serde(skip)]
    pub dir: PathBuf,
    #[serde(skip)]
    pub source: Option<PathBuf>,
    #[serde(skip)]
    pub media: Option<MediaInfo>,
    /// Image size in pixels.
    #[serde(skip)]
    pub image_size: Option<(u32, u32)>,
    #[serde(skip)]
    pub state: Option<LayerState>,
}

fn is_zero(v: &f64) -> bool {
    *v == 0.0
}

impl Span {
    /// Seconds into the source at timeline time `t` (during the hold: the first frame).
    pub fn source_time(&self, t: f64) -> f64 {
        self.trim_in + (t - self.start - self.hold).max(0.0)
    }

    /// How many seconds of source to decode from timeline time `t` on
    /// (0 once `out` has passed: nothing is left to play).
    pub fn decode_len(&self, t: f64) -> f64 {
        let rest = self.end - t + 1.0;
        match self.trim_out {
            Some(out) => rest.min((out - self.source_time(t)).max(0.0)),
            None => rest,
        }
    }

    /// Where to start a video decoder at time `t`, and for how long. Past `out`
    /// it decodes just the last frame of the piece, which is then held.
    pub fn video_range(&self, t: f64, fps: f64) -> (f64, f64) {
        let frame = 1.0 / fps;
        match self.trim_out {
            Some(out) if self.source_time(t) > out - frame => {
                let seek = (out - frame).max(self.trim_in);
                (seek, out - seek)
            }
            _ => (self.source_time(t), self.decode_len(t)),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Problem {
    pub layer: Option<String>,
    pub message: String,
}

#[derive(Debug, Serialize, Default)]
pub struct Report {
    pub duration: f64,
    pub layers: Vec<Span>,
    pub errors: Vec<Problem>,
    pub warnings: Vec<Problem>,
}

impl Report {
    fn err(&mut self, layer: Option<&str>, msg: impl Into<String>) {
        self.errors.push(Problem {
            layer: layer.map(String::from),
            message: msg.into(),
        });
    }
    fn warn(&mut self, layer: Option<&str>, msg: impl Into<String>) {
        self.warnings.push(Problem {
            layer: layer.map(String::from),
            message: msg.into(),
        });
    }
}

pub fn analyze(project: &Project) -> Report {
    let mut r = Report::default();

    let render = &project.file.render;
    let [w, h] = render.resolution;
    if w == 0 || h == 0 {
        r.err(None, "project.toml: resolution must not be zero");
    }
    if w % 2 == 1 || h % 2 == 1 {
        r.warn(None, "project.toml: odd resolution, h264 does not like that");
    }
    if !(render.fps > 0.0) {
        r.err(None, "project.toml: fps must be greater than zero");
    }
    if !matches!(render.codec.as_str(), "h264" | "h265") {
        r.err(None, format!("project.toml: codec = \"{}\": expected h264 or h265", render.codec));
    }
    if !matches!(render.quality.as_str(), "low" | "medium" | "high" | "lossless") {
        r.err(
            None,
            format!("project.toml: quality = \"{}\": expected low, medium, high or lossless", render.quality),
        );
    }
    if let Some(crf) = render.crf {
        if crf > 51 {
            r.err(None, format!("project.toml: crf = {crf}: expected 0..51"));
        }
    }

    let layers = match project.layers() {
        Ok(l) => l,
        Err(e) => {
            r.err(None, format!("{e:#}"));
            return r;
        }
    };
    if layers.is_empty() {
        r.warn(None, "project has no layers — add one: clve new layer <type> <name> <content>");
    }

    for layer in &layers {
        if let Some(span) = resolve(project, layer, &mut r) {
            r.layers.push(span);
        }
    }

    find_overlaps(&mut r);
    r.duration = r.layers.iter().map(|s| s.end).fold(0.0, f64::max);
    if render.fps > 0.0 {
        try_scripts(project, &mut r);
    }
    r
}

/// Loads every layer.lua and runs frame() at a few points of the layer,
/// so script errors show up now and not halfway through a long render.
fn try_scripts(project: &Project, r: &mut Report) {
    let render = &project.file.render;
    let env = Env {
        root: project.root.clone(),
        width: render.resolution[0],
        height: render.resolution[1],
        fps: render.fps,
        duration: r.duration,
    };
    let mut problems = Vec::new();
    for span in &r.layers {
        let Some(base) = &span.state else { continue };
        let script = match Script::load(span, &env) {
            Ok(Some(s)) => s,
            Ok(None) => continue,
            Err(e) => {
                problems.push((span.name.clone(), format!("{e:#}")));
                continue;
            }
        };
        let len = span.end - span.start;
        let last = (len - 1.0 / render.fps).max(0.0);
        for t in [0.0, len * 0.25, len * 0.5, len * 0.75, last] {
            let mut st = base.clone();
            if let Err(e) = script.apply(t, span.start + t, &mut st) {
                problems.push((span.name.clone(), format!("{e:#} (at t = {})", time::format(t))));
                break;
            }
            if span.kind.is_audio_only() && !st.fx.is_empty() {
                problems.push((span.name.clone(), "pixel effects need a visual layer, this one is audio".into()));
                break;
            }
        }
    }
    for (layer, msg) in problems {
        r.err(Some(&layer), msg);
    }
}

/// Works out where a layer sits on the timeline. Returns None if the layer is
/// too broken to place (the error is already recorded).
fn resolve(project: &Project, layer: &Layer, r: &mut Report) -> Option<Span> {
    let c = &layer.config;
    let name = layer.name.as_str();
    let canvas = (project.file.render.resolution[0], project.file.render.resolution[1]);

    let state = match LayerState::from_config(c, canvas) {
        Ok(s) => Some(s),
        Err(e) => {
            r.err(Some(name), format!("{e:#}"));
            None
        }
    };

    // content, by layer type
    let mut source = None;
    let mut media = None;
    let mut image_size = None;
    if c.kind.has_source() {
        match &c.source {
            None => r.err(Some(name), format!("layer of type {} needs a source", c.kind)),
            Some(src) => {
                let path = project.root.join(src);
                if !path.is_file() {
                    r.err(Some(name), format!("file not found: {src}"));
                } else if c.kind == LayerType::Image {
                    match image::image_dimensions(&path) {
                        Ok(size) => image_size = Some(size),
                        Err(e) => r.err(Some(name), format!("can't read image {src}: {e}")),
                    }
                } else {
                    match probe::info(&path) {
                        Ok(info) => {
                            if c.kind == LayerType::Video && info.video.is_none() {
                                r.err(Some(name), format!("{src} has no video stream"));
                            }
                            if c.kind == LayerType::Audio && !info.has_audio {
                                r.err(Some(name), format!("{src} has no audio stream"));
                            }
                            media = Some(info);
                        }
                        Err(e) => r.err(Some(name), format!("{e:#}")),
                    }
                }
                source = Some(path);
            }
        }
    }
    if c.kind == LayerType::Text && c.content.as_deref().unwrap_or("").is_empty() {
        r.err(Some(name), "text layer has empty content");
    }
    if c.kind == LayerType::Color && c.color.is_none() {
        r.err(Some(name), "color layer needs color = \"#rrggbb\"");
    }
    if !layer.dir.join("layer.lua").is_file() {
        r.warn(Some(name), "no layer.lua — the layer will be static");
    }

    let start = match time::parse(&c.start) {
        Ok(v) => v,
        Err(e) => {
            r.err(Some(name), format!("start: {e}"));
            return None;
        }
    };
    let trim_in = match c.trim_in.as_deref().map(time::parse).transpose() {
        Ok(v) => v.unwrap_or(0.0),
        Err(e) => {
            r.err(Some(name), format!("in: {e}"));
            return None;
        }
    };
    if trim_in > 0.0 && !c.kind.has_natural_duration() {
        r.warn(Some(name), format!("in has no effect on a layer of type {}", c.kind));
    }
    let trim_out = match c.trim_out.as_deref().map(time::parse).transpose() {
        Ok(v) => v,
        Err(e) => {
            r.err(Some(name), format!("out: {e}"));
            return None;
        }
    };
    if let Some(out) = trim_out {
        if !c.kind.has_natural_duration() {
            r.warn(Some(name), format!("out has no effect on a layer of type {}", c.kind));
        } else if out <= trim_in {
            r.err(Some(name), format!("out = {} must be after in = {}", time::format(out), time::format(trim_in)));
            return None;
        }
    }
    let trim_out = if c.kind.has_natural_duration() { trim_out } else { None };
    let hold = match c.hold_start.as_deref().map(time::parse).transpose() {
        Ok(v) => v.unwrap_or(0.0),
        Err(e) => {
            r.err(Some(name), format!("hold_start: {e}"));
            return None;
        }
    };
    if hold > 0.0 && !c.kind.has_natural_duration() {
        r.warn(Some(name), format!("hold_start has no effect on a layer of type {}", c.kind));
    }
    let hold = if c.kind.has_natural_duration() { hold } else { 0.0 };
    if c.kind != LayerType::Video && c.kind != LayerType::Audio
        && (c.lowpass.unwrap_or(0.0) > 0.0 || c.highpass.unwrap_or(0.0) > 0.0)
    {
        r.warn(Some(name), format!("lowpass/highpass have no effect on a layer of type {}", c.kind));
    }
    if c.kind == LayerType::Adjust && c.blend.is_some() {
        r.warn(Some(name), "blend has no effect on an adjust layer");
    }
    // `out` shortens the source like a shorter file would
    let source_len = media.as_ref().map(|m| match trim_out {
        Some(out) => out.min(m.duration),
        None => m.duration,
    });

    let duration = if c.duration.trim() == "full" {
        if !c.kind.has_natural_duration() {
            r.err(
                Some(name),
                format!(
                    "layer of type {} has no natural length, set duration explicitly, e.g. \"5s\"",
                    c.kind
                ),
            );
            return None;
        }
        let len = source_len?; // file error already recorded
        if trim_in >= len {
            r.err(
                Some(name),
                format!(
                    "in = {} is past the end of the file (length {})",
                    time::format(trim_in),
                    time::format(len)
                ),
            );
            return None;
        }
        hold + len - trim_in
    } else {
        match time::parse(&c.duration) {
            Ok(d) if d > 0.0 => {
                // with `out` the hold at the end is on purpose, no warning
                if let (Some(len), None) = (source_len, trim_out) {
                    if trim_in + (d - hold) > len + 0.05 {
                        r.warn(
                            Some(name),
                            format!(
                                "source is too short: needs {} from {}, file has {} (last frame will be held)",
                                time::format(d - hold),
                                time::format(trim_in),
                                time::format(len)
                            ),
                        );
                    }
                }
                d
            }
            Ok(_) => {
                r.err(Some(name), "duration must be greater than zero");
                return None;
            }
            Err(e) => {
                r.err(Some(name), format!("duration: {e}"));
                return None;
            }
        }
    };

    Some(Span {
        name: name.to_string(),
        kind: c.kind,
        z: c.z,
        start,
        end: start + duration,
        trim_in,
        trim_out,
        hold,
        dir: layer.dir.clone(),
        source,
        media,
        image_size,
        state,
    })
}

/// Two layers at the same height cannot play at the same time.
/// Audio layers live on their own tracks: music at z=0 does not clash with video at z=0.
fn find_overlaps(r: &mut Report) {
    const EPS: f64 = 1e-6;
    let mut sorted = r.layers.clone();
    sorted.sort_by(|a, b| {
        (a.kind.is_audio_only(), a.z)
            .cmp(&(b.kind.is_audio_only(), b.z))
            .then(a.start.total_cmp(&b.start))
    });
    let mut found = Vec::new();
    for (i, a) in sorted.iter().enumerate() {
        for b in &sorted[i + 1..] {
            if (b.kind.is_audio_only(), b.z) != (a.kind.is_audio_only(), a.z) {
                break;
            }
            if b.start >= a.end - EPS {
                break;
            }
            let from = b.start;
            let to = a.end.min(b.end);
            let track = if a.kind.is_audio_only() { "audio z" } else { "z" };
            found.push(format!(
                "layers overlap on {track}={}\n  {:<12} {} – {}\n  {:<12} {} – {}\n  overlap {} at {}",
                a.z,
                a.name,
                time::format(a.start),
                time::format(a.end),
                b.name,
                time::format(b.start),
                time::format(b.end),
                time::format(to - from),
                time::format(from),
            ));
        }
    }
    for m in found {
        r.err(None, m);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start: f64, end: f64, trim_in: f64, trim_out: Option<f64>, hold: f64) -> Span {
        Span {
            name: "x".into(),
            kind: LayerType::Video,
            z: 0,
            start,
            end,
            trim_in,
            trim_out,
            hold,
            dir: PathBuf::new(),
            source: None,
            media: None,
            image_size: None,
            state: None,
        }
    }

    #[test]
    fn hold_start_keeps_the_first_frame() {
        let s = span(2.0, 6.0, 1.0, None, 0.5);
        assert_eq!(s.source_time(2.0), 1.0);
        assert_eq!(s.source_time(2.4), 1.0);
        assert_eq!(s.source_time(3.5), 2.0);
    }

    #[test]
    fn out_limits_what_is_decoded() {
        let s = span(0.0, 10.0, 1.0, Some(3.0), 0.0);
        assert_eq!(s.decode_len(0.0), 2.0);
        assert_eq!(s.decode_len(1.5), 0.5);
        // past `out` nothing more is read: the last frame is held
        assert_eq!(s.decode_len(5.0), 0.0);
        let free = span(0.0, 10.0, 1.0, None, 0.0);
        assert_eq!(free.decode_len(4.0), 7.0);
    }

    #[test]
    fn starting_after_out_shows_the_last_frame() {
        let s = span(0.0, 10.0, 1.0, Some(3.0), 0.0);
        let (seek, len) = s.video_range(5.0, 25.0);
        assert!((seek - 2.96).abs() < 1e-9 && (len - 0.04).abs() < 1e-9);
        assert_eq!(s.video_range(0.5, 25.0), (1.5, 1.5));
    }
}
