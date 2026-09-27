//! Text rendering with cosmic-text: shaping, system fonts, fallback for any script.

use crate::scene::{Align, LayerState};
use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache};
use tiny_skia::Pixmap;

pub struct TextRenderer {
    fonts: FontSystem,
    cache: SwashCache,
}

/// Everything that changes how the text looks. If it did not change since the
/// last frame, the cached pixmap is reused.
#[derive(Clone, PartialEq)]
pub struct TextKey {
    text: String,
    font: String,
    px: u32,
    align: Align,
    color: [u8; 4],
}

impl TextKey {
    pub fn new(st: &LayerState, px_size: f32) -> TextKey {
        TextKey {
            text: st.text.clone(),
            font: st.font.clone(),
            px: (px_size * 64.0).round() as u32,
            align: st.align,
            color: st.color,
        }
    }
}

impl TextRenderer {
    pub fn new() -> TextRenderer {
        TextRenderer {
            fonts: FontSystem::new(),
            cache: SwashCache::new(),
        }
    }

    /// Whether a font family is installed (generic names always are).
    pub fn has_family(&self, name: &str) -> bool {
        if generic(name).is_some() {
            return true;
        }
        self.fonts
            .db()
            .faces()
            .any(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)))
    }

    /// Renders text into a tight premultiplied pixmap. None for empty text.
    pub fn render(&mut self, key: &TextKey) -> Option<Pixmap> {
        if key.text.trim().is_empty() {
            return None;
        }
        let px = key.px as f32 / 64.0;
        let metrics = Metrics::new(px, (px * 1.2).ceil());
        let mut buffer = Buffer::new(&mut self.fonts, metrics);
        let mut b = buffer.borrow_with(&mut self.fonts);

        let mut attrs = Attrs::new();
        attrs.family = generic(&key.font).unwrap_or(Family::Name(&key.font));
        let align = match key.align {
            Align::Left => cosmic_text::Align::Left,
            Align::Center => cosmic_text::Align::Center,
            Align::Right => cosmic_text::Align::Right,
        };

        b.set_size(None, None);
        b.set_text(&key.text, &attrs, Shaping::Advanced, Some(align));
        b.shape_until_scroll(true);
        let (mut w, mut h) = (0.0f32, 0.0f32);
        for run in b.layout_runs() {
            w = w.max(run.line_w);
            h = h.max(run.line_top + run.line_height);
        }
        if w <= 0.0 || h <= 0.0 {
            return None;
        }
        // lay out again with a fixed width so center/right alignment has something to align to
        b.set_size(Some(w.ceil()), None);
        b.shape_until_scroll(true);

        // glyphs can poke out of the line box (italics, accents), so leave a margin
        let pad = (px * 0.25).ceil() as i32;
        let pw = w.ceil() as i32 + pad * 2;
        let ph = h.ceil() as i32 + pad * 2;
        let mut pixmap = Pixmap::new(pw as u32, ph as u32)?;
        let data = pixmap.data_mut();

        let [r, g, bl, a] = key.color;
        b.draw(&mut self.cache, Color::rgba(r, g, bl, a), |x, y, rw, rh, c| {
            let alpha = c.a() as u32;
            if alpha == 0 {
                return;
            }
            for dy in 0..rh as i32 {
                for dx in 0..rw as i32 {
                    let px = x + dx + pad;
                    let py = y + dy + pad;
                    if px < 0 || py < 0 || px >= pw || py >= ph {
                        continue;
                    }
                    let i = ((py * pw + px) * 4) as usize;
                    // premultiplied source-over
                    let src = [
                        c.r() as u32 * alpha / 255,
                        c.g() as u32 * alpha / 255,
                        c.b() as u32 * alpha / 255,
                        alpha,
                    ];
                    let inv = 255 - alpha;
                    for k in 0..4 {
                        data[i + k] = (src[k] + data[i + k] as u32 * inv / 255) as u8;
                    }
                }
            }
        });
        Some(pixmap)
    }
}

fn generic(name: &str) -> Option<Family<'static>> {
    Some(match name {
        "sans-serif" | "sans" => Family::SansSerif,
        "serif" => Family::Serif,
        "monospace" | "mono" => Family::Monospace,
        _ => return None,
    })
}
