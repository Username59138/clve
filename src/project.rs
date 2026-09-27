//! Project model: project.toml and layers/<name>/layer.toml.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

pub const PROJECT_FILE: &str = "project.toml";
pub const LAYERS_DIR: &str = "layers";
pub const MEDIA_DIR: &str = "media";
pub const EFFECTS_DIR: &str = "effects";

// ---------- project.toml ----------

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectFile {
    pub project: ProjectMeta,
    pub render: RenderConfig,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RenderConfig {
    pub resolution: [u32; 2],
    pub fps: f64,
    #[serde(default = "default_codec")]
    pub codec: String,
    #[serde(default = "default_quality")]
    pub quality: String,
    #[serde(default = "default_output")]
    pub output: String,
    /// Overrides `quality` for those who know x264.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crf: Option<u32>,
}

fn default_codec() -> String {
    "h264".into()
}
fn default_quality() -> String {
    "high".into()
}
fn default_output() -> String {
    "out/".into()
}

// ---------- layer.toml ----------

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum LayerType {
    #[default]
    Video,
    Image,
    Text,
    Audio,
    Color,
}

impl LayerType {
    /// Whether the layer has a source file.
    pub fn has_source(self) -> bool {
        matches!(self, LayerType::Video | LayerType::Image | LayerType::Audio)
    }
    /// Whether duration can be "full" (taken from the file).
    pub fn has_natural_duration(self) -> bool {
        matches!(self, LayerType::Video | LayerType::Audio)
    }
    /// Audio takes no space in the picture, so it has its own z space.
    pub fn is_audio_only(self) -> bool {
        self == LayerType::Audio
    }
}

impl fmt::Display for LayerType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let s = match self {
            LayerType::Video => "video",
            LayerType::Image => "image",
            LayerType::Text => "text",
            LayerType::Audio => "audio",
            LayerType::Color => "color",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LayerFile {
    pub layer: LayerConfig,
}

/// How a video or image is sized to the canvas before `scale` is applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    /// Whole picture visible, letterboxed if needed.
    Contain,
    /// Fills the canvas, cropping what does not fit.
    Cover,
    /// Fills the canvas, ignoring aspect ratio.
    Stretch,
    /// Original pixel size.
    None,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LayerConfig {
    #[serde(rename = "type")]
    pub kind: LayerType,
    /// File path relative to the project root (video/image/audio).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Text (text).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Color "#rrggbb" (color, text).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub font: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<f64>,
    /// Where the layer starts on the main timeline.
    #[serde(default = "zero_time")]
    pub start: String,
    /// "full" or a time ("5s", "00:12").
    pub duration: String,
    /// Where in the source to start video/audio from.
    #[serde(rename = "in", skip_serializing_if = "Option::is_none")]
    pub trim_in: Option<String>,
    /// Height: higher value is drawn on top.
    #[serde(default)]
    pub z: i32,

    // ----- transform: defaults put the layer in the middle of the frame -----
    #[serde(skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    /// Degrees, clockwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rotation: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f64>,
    /// "center", "top-left", "bottom-right", ...
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
    /// video/image: contain (video default), cover, stretch, none (image default).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fit: Option<Fit>,

    // ----- type-specific -----
    /// video/audio: 1.0 = original loudness.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<f64>,
    /// text: "left", "center", "right".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub align: Option<String>,
}

fn zero_time() -> String {
    "0s".into()
}

// ---------- loaded project ----------

pub struct Project {
    pub root: PathBuf,
    pub file: ProjectFile,
}

pub struct Layer {
    pub name: String,
    pub dir: PathBuf,
    pub config: LayerConfig,
}

impl Project {
    /// Looks for project.toml in the current folder and upwards, like cargo.
    pub fn discover() -> Result<Project> {
        let cwd = std::env::current_dir()?;
        let mut dir: &Path = &cwd;
        loop {
            if dir.join(PROJECT_FILE).is_file() {
                return Project::load(dir);
            }
            match dir.parent() {
                Some(p) => dir = p,
                None => bail!(
                    "not inside a clve project ({PROJECT_FILE} not found here or in any parent folder)\n\
                     create one: clve new video <name>"
                ),
            }
        }
    }

    pub fn load(root: &Path) -> Result<Project> {
        let path = root.join(PROJECT_FILE);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("can't read {}", path.display()))?;
        let file: ProjectFile =
            toml::from_str(&text).with_context(|| format!("error in {}", path.display()))?;
        Ok(Project {
            root: root.to_path_buf(),
            file,
        })
    }

    pub fn layers_dir(&self) -> PathBuf {
        self.root.join(LAYERS_DIR)
    }

    /// All layers in alphabetical order. A broken layer.toml is an error naming its path.
    pub fn layers(&self) -> Result<Vec<Layer>> {
        let dir = self.layers_dir();
        let mut out = Vec::new();
        if !dir.is_dir() {
            return Ok(out);
        }
        let mut entries: Vec<_> = std::fs::read_dir(&dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            out.push(Layer::load(&name, &e.path())?);
        }
        Ok(out)
    }
}

impl Layer {
    pub fn load(name: &str, dir: &Path) -> Result<Layer> {
        let path = dir.join("layer.toml");
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("layer \"{name}\": can't read {}", path.display()))?;
        let file: LayerFile = toml::from_str(&text)
            .with_context(|| format!("layer \"{name}\": error in {}", path.display()))?;
        Ok(Layer {
            name: name.to_string(),
            dir: dir.to_path_buf(),
            config: file.layer,
        })
    }
}

/// Layer/project names become folder names, so keep them simple.
pub fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
        && !name.starts_with('-');
    if !ok {
        bail!("invalid name \"{name}\": use only letters, digits, _ and -");
    }
    Ok(())
}
