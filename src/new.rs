//! clve new video / clve new layer / clve new effect

use crate::project::*;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

pub fn video(name: &str) -> Result<()> {
    validate_name(name)?;
    let root = PathBuf::from(name);
    if root.exists() {
        bail!("folder \"{name}\" already exists");
    }
    for d in [LAYERS_DIR, MEDIA_DIR, EFFECTS_DIR] {
        std::fs::create_dir_all(root.join(d))?;
    }
    let project = ProjectFile {
        project: ProjectMeta { name: name.into() },
        render: RenderConfig {
            resolution: [1920, 1080],
            fps: 30.0,
            codec: "h264".into(),
            quality: "high".into(),
            output: "out/".into(),
        },
    };
    std::fs::write(root.join(PROJECT_FILE), toml::to_string(&project)?)?;
    std::fs::write(root.join(".gitignore"), "out/\n")?;

    println!("created project {name}");
    println!();
    println!("  cd {name}");
    println!("  clve new layer video intro ~/clip.mp4");
    println!("  clve render");
    Ok(())
}

pub fn layer(kind: LayerType, name: &str, value: &str, copy: bool) -> Result<()> {
    validate_name(name)?;
    let project = Project::discover()?;
    let dir = project.layers_dir().join(name);
    if dir.exists() {
        bail!("layer \"{name}\" already exists (layers/{name})");
    }

    let mut cfg = LayerConfig {
        kind,
        source: None,
        content: None,
        color: None,
        font: None,
        size: None,
        start: "0s".into(),
        duration: "5s".into(),
        trim_in: None,
        z: next_free_z(&project, kind)?,
    };

    match kind {
        LayerType::Video | LayerType::Audio | LayerType::Image => {
            cfg.source = Some(import_media(&project, Path::new(value), copy)?);
            if kind != LayerType::Image {
                cfg.duration = "full".into();
            }
        }
        LayerType::Text => {
            cfg.content = Some(value.into());
            cfg.font = Some("sans-serif".into());
            cfg.size = Some(72.0);
            cfg.color = Some("#ffffff".into());
            cfg.duration = "4s".into();
        }
        LayerType::Color => {
            cfg.color = Some(value.into());
        }
    }

    std::fs::create_dir_all(&dir)?;
    std::fs::write(
        dir.join("layer.toml"),
        toml::to_string(&LayerFile { layer: cfg })?,
    )?;
    std::fs::write(dir.join("layer.lua"), lua_template(kind))?;

    println!("created layer {name} ({kind}) → layers/{name}/");
    Ok(())
}

pub fn effect(name: &str) -> Result<()> {
    validate_name(name)?;
    let project = Project::discover()?;
    let dir = project.root.join(EFFECTS_DIR);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.lua"));
    if path.exists() {
        bail!("effect \"{name}\" already exists (effects/{name}.lua)");
    }
    std::fs::write(
        &path,
        format!(
            "-- effect {name}\n\
             -- usage in layer.lua: layer:effect(\"{name}\", {{ power = 1 }})\n\n\
             params = {{\n  power = 1.0,\n}}\n\n\
             function apply(t, layer, p)\n  -- layer.opacity = layer.opacity * p.power\nend\n"
        ),
    )?;
    println!("created effect {name} → effects/{name}.lua");
    Ok(())
}

/// Copies the file into media/ (or keeps an absolute path with --link) and
/// returns the path relative to the project root.
fn import_media(project: &Project, src: &Path, copy: bool) -> Result<String> {
    if !src.is_file() {
        bail!("file not found: {}", src.display());
    }
    if !copy {
        let abs = std::fs::canonicalize(src)?;
        return Ok(abs.to_string_lossy().into_owned());
    }
    let media = project.root.join(MEDIA_DIR);
    std::fs::create_dir_all(&media)?;
    let file_name = src
        .file_name()
        .context("file has no name")?
        .to_string_lossy()
        .into_owned();
    let dest = media.join(&file_name);

    let same = dest.exists()
        && std::fs::canonicalize(&dest).ok() == std::fs::canonicalize(src).ok();
    if dest.exists() && !same {
        // same file already imported — reuse it; never overwrite a different one
        let a = std::fs::metadata(&dest)?.len();
        let b = std::fs::metadata(src)?.len();
        if a != b {
            bail!(
                "media/ already has a different {file_name}; rename the file or use --link"
            );
        }
    } else if !dest.exists() {
        std::fs::copy(src, &dest)
            .with_context(|| format!("failed to copy {}", src.display()))?;
    }
    Ok(format!("{MEDIA_DIR}/{file_name}"))
}

/// New visual layers go on top of everything; audio goes to the next free audio track.
fn next_free_z(project: &Project, kind: LayerType) -> Result<i32> {
    let max = project
        .layers()?
        .iter()
        .filter(|l| l.config.kind.is_audio_only() == kind.is_audio_only())
        .map(|l| l.config.z)
        .max();
    Ok(max.map_or(0, |z| z + 1))
}

fn lua_template(kind: LayerType) -> String {
    let specific = match kind {
        LayerType::Video => "  -- layer.volume = 1.0\n",
        LayerType::Audio => "  -- layer.volume = fade_in(t, 0, 1.0)\n",
        LayerType::Text => "  -- layer.text = typewriter(layer.content, t, 0.05)\n",
        LayerType::Color => "  -- layer.color = \"#101010\"\n",
        LayerType::Image => "",
    };
    format!(
        "-- called on every frame\n\
         -- t: time since the layer started (seconds)\n\
         function frame(t, layer)\n\
         \x20 -- layer.x, layer.y = project.width / 2, project.height / 2\n\
         \x20 -- layer.scale = 1.0\n\
         \x20 -- layer.opacity = ease_out(t, 0, 0.5)\n\
         {specific}\
         end\n"
    )
}
