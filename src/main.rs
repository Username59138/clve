mod check;
mod new;
mod probe;
mod project;
mod render;
mod scene;
mod script;
mod time;
mod timeline;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use project::{LayerType, Project};
use std::process::ExitCode;

/// clve — Command Line Video Editor. Video editing as a project: folders, TOML and Lua.
#[derive(Parser)]
#[command(name = "clve", version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a project, layer or effect
    New {
        #[command(subcommand)]
        what: NewCmd,
    },
    /// List layers or effects
    List {
        #[command(subcommand)]
        what: ListCmd,
    },
    /// Remove a layer or effect
    Rm {
        #[command(subcommand)]
        what: RmCmd,
    },
    /// Check the project: files, durations, layer overlaps
    Check {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Render the project to a video file (or a single frame to PNG)
    Render {
        /// Output file (default: <render.output>/<name>.mp4)
        #[arg(short, long)]
        output: Option<std::path::PathBuf>,
        /// Start of the range to render, e.g. 10s or 01:30
        #[arg(long, value_parser = parse_time)]
        from: Option<f64>,
        /// End of the range to render
        #[arg(long, value_parser = parse_time)]
        to: Option<f64>,
        /// Fast low-quality render (480p, ultrafast preset)
        #[arg(long)]
        preview: bool,
        /// Render only the frame at this time into a PNG
        #[arg(long, value_parser = parse_time, conflicts_with_all = ["from", "to"])]
        frame: Option<f64>,
        /// Output width in pixels, e.g. --width 640 for a small GIF
        #[arg(long)]
        width: Option<u32>,
    },
    /// Draw the timeline in the terminal
    Timeline {
        /// Width in characters
        #[arg(short, long, default_value_t = 60)]
        width: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum NewCmd {
    /// New project: clve new video <name>
    Video { name: String },
    /// New layer: clve new layer <type> <name> <file | text | #color>
    Layer {
        #[arg(value_enum)]
        kind: LayerType,
        name: String,
        /// File path (video, image, audio), text (text) or color (color)
        value: String,
        /// Don't copy the file into media/, reference it by absolute path
        #[arg(long)]
        link: bool,
    },
    /// New effect in effects/: clve new effect <name>
    Effect { name: String },
}

#[derive(Subcommand)]
enum ListCmd {
    /// Project layers
    Layers {
        #[arg(long)]
        json: bool,
    },
    /// Project effects (effects/)
    Effects,
}

#[derive(Subcommand)]
enum RmCmd {
    /// Remove a layer (the layers/<name> folder)
    Layer { name: String },
    /// Remove an effect (effects/<name>.lua)
    Effect { name: String },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    match cli.cmd {
        Cmd::New { what } => match what {
            NewCmd::Video { name } => new::video(&name)?,
            NewCmd::Layer {
                kind,
                name,
                value,
                link,
            } => new::layer(kind, &name, &value, !link)?,
            NewCmd::Effect { name } => new::effect(&name)?,
        },
        Cmd::List { what } => match what {
            ListCmd::Layers { json } => list_layers(json)?,
            ListCmd::Effects => list_effects()?,
        },
        Cmd::Rm { what } => rm(what)?,
        Cmd::Check { json } => return check(json),
        Cmd::Render {
            output,
            from,
            to,
            preview,
            frame,
            width,
        } => render::run(
            &Project::discover()?,
            render::Options {
                output,
                from,
                to,
                preview,
                frame,
                width,
            },
        )?,
        Cmd::Timeline { width, json } => {
            let report = check::analyze(&Project::discover()?);
            if json {
                println!("{}", serde_json::to_string_pretty(&report.layers)?);
            } else {
                print!("{}", timeline::draw(&report, width));
                if !report.errors.is_empty() {
                    eprintln!("\n{} error(s), details: clve check", report.errors.len());
                }
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn parse_time(s: &str) -> Result<f64, String> {
    time::parse(s).map_err(|e| e.to_string())
}

fn check(json: bool) -> Result<ExitCode> {
    let report = check::analyze(&Project::discover()?);
    let failed = !report.errors.is_empty();
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let show = |kind: &str, p: &check::Problem| match &p.layer {
            Some(l) => eprintln!("{kind}: [{l}] {}", p.message),
            None => eprintln!("{kind}: {}", p.message),
        };
        for w in &report.warnings {
            show("warning", w);
        }
        for e in &report.errors {
            show("error", e);
        }
        if failed {
            eprintln!("\ncheck failed: {} error(s)", report.errors.len());
        } else {
            println!(
                "ok: {} layer(s), length {}",
                report.layers.len(),
                time::format(report.duration)
            );
        }
    }
    Ok(if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn list_layers(json: bool) -> Result<()> {
    let project = Project::discover()?;
    let layers = project.layers()?;
    if json {
        let v: Vec<_> = layers
            .iter()
            .map(|l| serde_json::json!({ "name": l.name, "layer": l.config }))
            .collect();
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    if layers.is_empty() {
        println!("no layers");
        return Ok(());
    }
    for l in layers {
        let c = &l.config;
        let what = c
            .source
            .as_deref()
            .or(c.content.as_deref())
            .or(c.color.as_deref())
            .unwrap_or("");
        println!(
            "{:<14} {:<6} z={:<3} start={:<7} dur={:<6} {}",
            l.name, c.kind.to_string(), c.z, c.start, c.duration, what
        );
    }
    Ok(())
}

fn list_effects() -> Result<()> {
    let project = Project::discover()?;
    let dir = project.root.join(project::EFFECTS_DIR);
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| {
                    let p = e.path();
                    if p.extension()? != "lua" {
                        return None;
                    }
                    Some(p.file_stem()?.to_string_lossy().into_owned())
                })
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    for n in &names {
        let overrides = script::BUILTIN_EFFECTS.iter().any(|(b, _)| b == n);
        let note = if overrides { "  (overrides built-in)" } else { "" };
        println!("{n:<12} effects/{n}.lua{note}");
    }
    for (n, _) in script::BUILTIN_EFFECTS {
        if !names.iter().any(|m| m == n) {
            println!("{n:<12} built-in");
        }
    }
    Ok(())
}

fn rm(what: RmCmd) -> Result<()> {
    let project = Project::discover()?;
    match what {
        RmCmd::Layer { name } => {
            project::validate_name(&name)?;
            let dir = project.layers_dir().join(&name);
            if !dir.is_dir() {
                bail!("no layer named \"{name}\"");
            }
            std::fs::remove_dir_all(&dir)?;
            println!("removed layer {name} (files in media/ left untouched)");
        }
        RmCmd::Effect { name } => {
            project::validate_name(&name)?;
            let path = project.root.join(project::EFFECTS_DIR).join(format!("{name}.lua"));
            if !path.is_file() {
                bail!("no effect named \"{name}\"");
            }
            std::fs::remove_file(&path)?;
            println!("removed effect {name}");
        }
    }
    Ok(())
}
