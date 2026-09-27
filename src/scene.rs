//! Per-frame layer state. Built from layer.toml; later Lua scripts modify it
//! every frame before the layer is drawn.

use crate::project::{Fit, LayerConfig, LayerType};
use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Debug, Clone)]
pub struct LayerState {
    /// Anchor position in project pixels.
    pub x: f64,
    pub y: f64,
    pub scale: f64,
    /// Degrees, clockwise.
    pub rotation: f64,
    pub opacity: f64,
    /// Anchor as a fraction of the layer size: (0,0) top-left, (0.5,0.5) center.
    pub anchor: (f64, f64),
    pub visible: bool,
    pub fit: Fit,
    pub volume: f64,
    // text
    pub text: String,
    pub font: String,
    pub size: f64,
    pub align: Align,
    /// RGBA, straight alpha.
    pub color: [u8; 4],
}

impl LayerState {
    pub fn from_config(c: &LayerConfig, canvas: (u32, u32)) -> Result<LayerState> {
        let anchor = match &c.anchor {
            Some(a) => parse_anchor(a)?,
            None => (0.5, 0.5),
        };
        let align = match c.align.as_deref() {
            None | Some("center") => Align::Center,
            Some("left") => Align::Left,
            Some("right") => Align::Right,
            Some(other) => bail!("align = \"{other}\": expected left, center or right"),
        };
        let color = match &c.color {
            Some(col) => parse_color(col)?,
            None => [255, 255, 255, 255],
        };
        let opacity = c.opacity.unwrap_or(1.0);
        if !(0.0..=1.0).contains(&opacity) {
            bail!("opacity = {opacity}: expected a value from 0 to 1");
        }
        let scale = c.scale.unwrap_or(1.0);
        if !(scale >= 0.0) {
            bail!("scale = {scale}: must not be negative");
        }
        let volume = c.volume.unwrap_or(1.0);
        if !(volume >= 0.0) {
            bail!("volume = {volume}: must not be negative");
        }
        let size = c.size.unwrap_or(72.0);
        if !(size > 0.0) {
            bail!("size = {size}: must be greater than zero");
        }
        let fit = c.fit.unwrap_or(match c.kind {
            LayerType::Video => Fit::Contain,
            _ => Fit::None,
        });
        Ok(LayerState {
            x: c.x.unwrap_or(canvas.0 as f64 / 2.0),
            y: c.y.unwrap_or(canvas.1 as f64 / 2.0),
            scale,
            rotation: c.rotation.unwrap_or(0.0),
            opacity,
            anchor,
            visible: true,
            fit,
            volume,
            text: c.content.clone().unwrap_or_default(),
            font: c.font.clone().unwrap_or_else(|| "sans-serif".into()),
            size,
            align,
            color,
        })
    }
}

pub fn parse_anchor(s: &str) -> Result<(f64, f64)> {
    Ok(match s {
        "center" => (0.5, 0.5),
        "top-left" => (0.0, 0.0),
        "top" => (0.5, 0.0),
        "top-right" => (1.0, 0.0),
        "left" => (0.0, 0.5),
        "right" => (1.0, 0.5),
        "bottom-left" => (0.0, 1.0),
        "bottom" => (0.5, 1.0),
        "bottom-right" => (1.0, 1.0),
        _ => bail!(
            "anchor = \"{s}\": expected center, top, bottom, left, right, top-left, top-right, bottom-left or bottom-right"
        ),
    })
}

/// "#rrggbb" or "#rrggbbaa".
pub fn parse_color(s: &str) -> Result<[u8; 4]> {
    let hex = s.strip_prefix('#').unwrap_or("");
    let ok = matches!(hex.len(), 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit());
    if !ok {
        bail!("color \"{s}\" is not #rrggbb or #rrggbbaa");
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
    let a = if hex.len() == 8 { byte(6) } else { 255 };
    Ok([byte(0), byte(2), byte(4), a])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors() {
        assert_eq!(parse_color("#ff8000").unwrap(), [255, 128, 0, 255]);
        assert_eq!(parse_color("#00000080").unwrap(), [0, 0, 0, 128]);
        assert!(parse_color("ff8000").is_err());
        assert!(parse_color("#ff80").is_err());
    }
}
