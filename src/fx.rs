//! Built-in pixel effects: what exists, which parameters each one takes and
//! their defaults. The same table is handed to Lua, so `layer:effect("blur", ...)`
//! validates parameters exactly like effects written in Lua.

use crate::scene::parse_color;
use anyhow::{anyhow, bail, Result};
use std::collections::HashMap;

pub enum Def {
    Num(f64),
    Color(&'static str),
}

pub struct Spec {
    pub name: &'static str,
    pub params: &'static [(&'static str, Def)],
}

use Def::{Color as C, Num as N};

/// Units: lengths are in project pixels, amounts are fractions (1 = unchanged
/// for brightness/contrast/saturation, 0 = no effect for the rest).
pub const SPECS: &[Spec] = &[
    // color
    Spec { name: "brightness", params: &[("amount", N(1.2))] },
    Spec { name: "contrast", params: &[("amount", N(1.2))] },
    Spec { name: "saturation", params: &[("amount", N(1.5))] },
    Spec { name: "hue", params: &[("degrees", N(30.0))] },
    Spec { name: "grayscale", params: &[("amount", N(1.0))] },
    Spec { name: "sepia", params: &[("amount", N(1.0))] },
    Spec { name: "invert", params: &[("amount", N(1.0))] },
    Spec { name: "tint", params: &[("color", C("#ff8800")), ("amount", N(0.3))] },
    Spec { name: "temperature", params: &[("amount", N(0.3))] },
    // blur and detail
    Spec { name: "blur", params: &[("radius", N(8.0))] },
    Spec { name: "sharpen", params: &[("amount", N(0.6)), ("radius", N(2.0))] },
    Spec { name: "pixelate", params: &[("size", N(16.0))] },
    // looks
    Spec { name: "vignette", params: &[("amount", N(0.5)), ("radius", N(0.5)), ("softness", N(0.5))] },
    Spec { name: "grain", params: &[("amount", N(0.06)), ("size", N(1.5)), ("speed", N(12.0)), ("seed", N(0.0))] },
    Spec { name: "glow", params: &[("radius", N(20.0)), ("amount", N(0.8)), ("threshold", N(0.6))] },
    Spec {
        name: "shadow",
        params: &[("x", N(8.0)), ("y", N(8.0)), ("blur", N(12.0)), ("color", C("#000000")), ("opacity", N(0.6))],
    },
    // compositing
    Spec {
        name: "chroma_key",
        params: &[("color", C("#00ff00")), ("similarity", N(0.25)), ("smoothness", N(0.08)), ("spill", N(0.5))],
    },
    Spec { name: "rounded", params: &[("radius", N(24.0))] },
    Spec { name: "crop", params: &[("left", N(0.0)), ("top", N(0.0)), ("right", N(0.0)), ("bottom", N(0.0))] },
];

/// Color as floats 0..1, straight alpha.
pub type Rgba = [f32; 4];

#[derive(Debug, Clone, PartialEq)]
pub enum PixelFx {
    Brightness(f32),
    Contrast(f32),
    Saturation(f32),
    Hue(f32),
    Grayscale(f32),
    Sepia(f32),
    Invert(f32),
    Tint { color: Rgba, amount: f32 },
    Temperature(f32),
    Blur(f32),
    Sharpen { amount: f32, radius: f32 },
    Pixelate(f32),
    Vignette { amount: f32, radius: f32, softness: f32 },
    Grain { amount: f32, size: f32, speed: f32, seed: f32 },
    Glow { radius: f32, amount: f32, threshold: f32 },
    Shadow { x: f32, y: f32, blur: f32, color: Rgba, opacity: f32 },
    ChromaKey { color: Rgba, similarity: f32, smoothness: f32, spill: f32 },
    Rounded(f32),
    Crop { left: f32, top: f32, right: f32, bottom: f32 },
}

/// A parameter value as it comes from Lua.
pub enum Val {
    Num(f64),
    Str(String),
}

pub fn spec(name: &str) -> Option<&'static Spec> {
    SPECS.iter().find(|s| s.name == name)
}

/// Builds an effect from its parameters (already merged with defaults by Lua).
pub fn build(name: &str, vals: &HashMap<String, Val>) -> Result<PixelFx> {
    let spec = spec(name).ok_or_else(|| anyhow!("unknown pixel effect '{name}'"))?;
    for key in vals.keys() {
        if !spec.params.iter().any(|(k, _)| k == key) {
            bail!("effect '{name}' has no parameter '{key}'");
        }
    }
    let num = |key: &str| -> Result<f32> {
        match vals.get(key) {
            Some(Val::Num(n)) if n.is_finite() => Ok(*n as f32),
            Some(Val::Num(_)) => bail!("effect '{name}': {key} is not a finite number"),
            Some(Val::Str(_)) => bail!("effect '{name}': {key} must be a number"),
            None => default_num(spec, key),
        }
    };
    let color = |key: &str| -> Result<Rgba> {
        let s = match vals.get(key) {
            Some(Val::Str(s)) => s.clone(),
            Some(Val::Num(_)) => bail!("effect '{name}': {key} must be a color like \"#ff8800\""),
            None => match spec.params.iter().find(|(k, _)| *k == key) {
                Some((_, Def::Color(c))) => c.to_string(),
                _ => bail!("effect '{name}': missing {key}"),
            },
        };
        let c = parse_color(&s).map_err(|e| anyhow!("effect '{name}': {key}: {e}"))?;
        Ok(c.map(|v| v as f32 / 255.0))
    };
    let non_negative = |key: &str| -> Result<f32> {
        let v = num(key)?;
        if v < 0.0 {
            bail!("effect '{name}': {key} must not be negative");
        }
        Ok(v)
    };

    Ok(match name {
        "brightness" => PixelFx::Brightness(non_negative("amount")?),
        "contrast" => PixelFx::Contrast(non_negative("amount")?),
        "saturation" => PixelFx::Saturation(non_negative("amount")?),
        "hue" => PixelFx::Hue(num("degrees")?),
        "grayscale" => PixelFx::Grayscale(num("amount")?.clamp(0.0, 1.0)),
        "sepia" => PixelFx::Sepia(num("amount")?.clamp(0.0, 1.0)),
        "invert" => PixelFx::Invert(num("amount")?.clamp(0.0, 1.0)),
        "tint" => PixelFx::Tint { color: color("color")?, amount: num("amount")?.clamp(0.0, 1.0) },
        "temperature" => PixelFx::Temperature(num("amount")?.clamp(-1.0, 1.0)),
        "blur" => PixelFx::Blur(non_negative("radius")?.min(500.0)),
        "sharpen" => PixelFx::Sharpen { amount: non_negative("amount")?, radius: non_negative("radius")?.min(50.0) },
        "pixelate" => PixelFx::Pixelate(non_negative("size")?),
        "vignette" => PixelFx::Vignette {
            amount: num("amount")?.clamp(0.0, 1.0),
            radius: non_negative("radius")?,
            softness: non_negative("softness")?,
        },
        "grain" => PixelFx::Grain {
            amount: non_negative("amount")?,
            size: non_negative("size")?.max(0.1),
            speed: non_negative("speed")?,
            seed: num("seed")?,
        },
        "glow" => PixelFx::Glow {
            radius: non_negative("radius")?.min(500.0),
            amount: non_negative("amount")?,
            threshold: num("threshold")?.clamp(0.0, 0.99),
        },
        "shadow" => PixelFx::Shadow {
            x: num("x")?,
            y: num("y")?,
            blur: non_negative("blur")?.min(500.0),
            color: color("color")?,
            opacity: num("opacity")?.clamp(0.0, 1.0),
        },
        "chroma_key" => PixelFx::ChromaKey {
            color: color("color")?,
            similarity: non_negative("similarity")?,
            smoothness: non_negative("smoothness")?,
            spill: num("spill")?.clamp(0.0, 1.0),
        },
        "rounded" => PixelFx::Rounded(non_negative("radius")?),
        "crop" => PixelFx::Crop {
            left: non_negative("left")?,
            top: non_negative("top")?,
            right: non_negative("right")?,
            bottom: non_negative("bottom")?,
        },
        _ => unreachable!("every spec has a builder"),
    })
}

fn default_num(spec: &Spec, key: &str) -> Result<f32> {
    match spec.params.iter().find(|(k, _)| *k == key) {
        Some((_, Def::Num(n))) => Ok(*n as f32),
        _ => bail!("effect '{}': missing {key}", spec.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_spec_builds_with_defaults() {
        for s in SPECS {
            build(s.name, &HashMap::new()).unwrap_or_else(|e| panic!("{}: {e}", s.name));
        }
    }

    #[test]
    fn rejects_bad_params() {
        let mut v = HashMap::new();
        v.insert("radius".to_string(), Val::Str("big".into()));
        assert!(build("blur", &v).is_err());
        let mut v = HashMap::new();
        v.insert("radus".to_string(), Val::Num(3.0));
        assert!(build("blur", &v).is_err());
        let mut v = HashMap::new();
        v.insert("color".to_string(), Val::Str("green".into()));
        assert!(build("chroma_key", &v).is_err());
    }
}
