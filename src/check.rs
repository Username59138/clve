//! Resolving layers into time spans and validating the project.

use crate::probe;
use crate::project::{Layer, LayerType, Project};
use crate::time;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct Span {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: LayerType,
    pub z: i32,
    pub start: f64,
    pub end: f64,
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

    let [w, h] = project.file.render.resolution;
    if w == 0 || h == 0 {
        r.err(None, "project.toml: resolution must not be zero");
    }
    if w % 2 == 1 || h % 2 == 1 {
        r.warn(None, "project.toml: odd resolution, h264 does not like that");
    }
    if !(project.file.render.fps > 0.0) {
        r.err(None, "project.toml: fps must be greater than zero");
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
    r
}

/// Works out where a layer sits on the timeline. Returns None if the layer is
/// too broken to place (the error is already recorded).
fn resolve(project: &Project, layer: &Layer, r: &mut Report) -> Option<Span> {
    let c = &layer.config;
    let name = layer.name.as_str();

    // content, by layer type
    let mut source_len = None;
    if c.kind.has_source() {
        match &c.source {
            None => {
                r.err(Some(name), format!("layer of type {} needs a source", c.kind));
            }
            Some(src) => {
                let path = project.root.join(src);
                if !path.is_file() {
                    r.err(Some(name), format!("file not found: {src}"));
                    } else if c.kind.has_natural_duration() {
                    match probe::duration(&path) {
                        Ok(d) => source_len = Some(d),
                        Err(e) => {
                            r.err(Some(name), format!("{e:#}"));
                                    }
                    }
                }
            }
        }
    }
    if c.kind == LayerType::Text && c.content.as_deref().unwrap_or("").is_empty() {
        r.err(Some(name), "text layer has empty content");
    }
    if matches!(c.kind, LayerType::Color | LayerType::Text) {
        if let Some(col) = &c.color {
            if !valid_color(col) {
                r.err(Some(name), format!("color \"{col}\" is not #rrggbb"));
            }
        } else if c.kind == LayerType::Color {
            r.err(Some(name), "color layer needs color = \"#rrggbb\"");
        }
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
        len - trim_in
    } else {
        match time::parse(&c.duration) {
            Ok(d) if d > 0.0 => {
                if let Some(len) = source_len {
                    if trim_in + d > len + 0.05 {
                        r.warn(
                            Some(name),
                            format!(
                                "source is too short: needs {} from {}, file has {}",
                                time::format(d),
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

fn valid_color(s: &str) -> bool {
    let hex = s.strip_prefix('#').unwrap_or("");
    matches!(hex.len(), 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit())
}
