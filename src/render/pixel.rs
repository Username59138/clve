//! Pixel effects on a layer's image, before it is placed into the frame.
//!
//! Works on premultiplied RGBA floats. Effects that spread outside the layer
//! (blur, glow, shadow) get a transparent margin, allocated once up front.
//! Rows are processed in parallel.

use super::gpu::{Gpu, Pass};
use crate::fx::{PixelFx, Rgba};
use crate::shader::Uniforms;
use anyhow::{bail, Result};
use rayon::prelude::*;
use tiny_skia::{IntSize, Pixmap};

type Px = [f32; 4];

/// Largest image an effect chain may produce (width * height).
const MAX_PIXELS: usize = 64 * 1024 * 1024;

pub struct Output {
    pub pixmap: Pixmap,
    /// Margin added on every side, in pixels of the input image.
    pub grow: f32,
}

/// Where the layer's actual content sits inside the image, in pixels.
/// Text images carry padding around the glyph box, for example.
#[derive(Clone, Copy)]
pub struct ContentRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

struct Img {
    w: usize,
    h: usize,
    px: Vec<Px>,
    content: ContentRect,
}

/// Time of the frame being drawn, for effects that change over time.
#[derive(Clone, Copy)]
pub struct FxTime {
    /// Seconds since the layer started.
    pub layer: f64,
    /// Seconds on the timeline.
    pub global: f64,
    pub fps: f64,
}

/// The GPU is opened the first time a shader effect runs.
pub type GpuSlot = Option<Gpu>;

/// `density`: image pixels per project pixel, so lengths in effect parameters
/// mean the same thing in a preview and a full render.
pub fn apply(
    src: &Pixmap,
    content: ContentRect,
    fx: &[PixelFx],
    density: f32,
    time: FxTime,
    gpu: &mut GpuSlot,
) -> Result<Output> {
    let grow = fx.iter().map(|f| extent(f, density)).sum::<f32>().ceil();
    let g = grow as usize;
    let (w, h) = (src.width() as usize + 2 * g, src.height() as usize + 2 * g);
    if w * h > MAX_PIXELS {
        bail!("effects make the layer too large ({w}x{h}); lower blur/glow/shadow sizes");
    }

    let mut img = Img {
        w,
        h,
        px: vec![[0.0; 4]; w * h],
        content: ContentRect { x: content.x + grow, y: content.y + grow, ..content },
    };
    let sw = src.width() as usize;
    let data = src.data();
    img.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        if y < g || y >= g + src.height() as usize {
            return;
        }
        let srow = &data[(y - g) * sw * 4..(y - g + 1) * sw * 4];
        for (x, p) in srow.chunks_exact(4).enumerate() {
            row[x + g] = [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, p[3] as f32 / 255.0];
        }
    });

    let mut i = 0;
    while i < fx.len() {
        // consecutive shaders run as one chain on the GPU
        let n = fx[i..].iter().take_while(|f| matches!(f, PixelFx::Shader { .. })).count();
        if n > 0 {
            run_shaders(&mut img, &fx[i..i + n], density, time, gpu)?;
            i += n;
        } else {
            run(&mut img, &fx[i], density, time);
            i += 1;
        }
    }

    let mut out = Pixmap::from_vec(vec![0; w * h * 4], IntSize::from_wh(w as u32, h as u32).unwrap()).unwrap();
    out.data_mut().par_chunks_mut(w * 4).zip(img.px.par_chunks(w)).for_each(|(drow, srow)| {
        for (d, p) in drow.chunks_exact_mut(4).zip(srow) {
            let a = p[3].clamp(0.0, 1.0);
            // keep the premultiplied invariant rgb <= a
            d[0] = (p[0].clamp(0.0, a) * 255.0 + 0.5) as u8;
            d[1] = (p[1].clamp(0.0, a) * 255.0 + 0.5) as u8;
            d[2] = (p[2].clamp(0.0, a) * 255.0 + 0.5) as u8;
            d[3] = (a * 255.0 + 0.5) as u8;
        }
    });
    Ok(Output { pixmap: out, grow })
}

/// How far an effect reaches past the layer edges, in image pixels.
fn extent(f: &PixelFx, density: f32) -> f32 {
    match f {
        PixelFx::Blur(r) => blur_extent(r * density),
        PixelFx::Glow { radius, .. } => blur_extent(radius * density),
        PixelFx::Shadow { x, y, blur, .. } => {
            blur_extent(blur * density) + (x.abs().max(y.abs()) * density).ceil()
        }
        PixelFx::Shader { margin, .. } => (margin * density).ceil(),
        _ => 0.0,
    }
}

fn run(img: &mut Img, f: &PixelFx, d: f32, time: FxTime) {
    let t = time.global;
    match *f {
        PixelFx::Brightness(a) => map_rgb(img, |c| c.map(|v| v * a)),
        PixelFx::Contrast(a) => map_rgb(img, |c| c.map(|v| (v - 0.5) * a + 0.5)),
        PixelFx::Saturation(a) => map_rgb(img, |c| {
            let l = luma(c);
            c.map(|v| l + (v - l) * a)
        }),
        PixelFx::Hue(deg) => {
            let m = hue_matrix(deg);
            map_rgb(img, |c| mul3(&m, c))
        }
        PixelFx::Grayscale(a) => map_rgb(img, |c| {
            let l = luma(c);
            c.map(|v| v + (l - v) * a)
        }),
        PixelFx::Sepia(a) => {
            const M: [[f32; 3]; 3] = [[0.393, 0.769, 0.189], [0.349, 0.686, 0.168], [0.272, 0.534, 0.131]];
            map_rgb(img, |c| {
                let s = mul3(&M, c);
                [0, 1, 2].map(|i| c[i] + (s[i] - c[i]) * a)
            })
        }
        PixelFx::Invert(a) => map_rgb(img, |c| c.map(|v| v + (1.0 - 2.0 * v) * a)),
        PixelFx::Tint { color, amount } => {
            let k = amount * color[3];
            map_rgb(img, |c| [0, 1, 2].map(|i| c[i] + (color[i] - c[i]) * k))
        }
        PixelFx::Temperature(a) => map_rgb(img, |c| [c[0] * (1.0 + 0.25 * a), c[1], c[2] * (1.0 - 0.25 * a)]),
        PixelFx::Blur(r) => gaussian(img, r * d),
        PixelFx::Sharpen { amount, radius } => sharpen(img, amount, radius * d),
        PixelFx::Pixelate(size) => pixelate(img, size * d),
        PixelFx::Vignette { amount, radius, softness } => vignette(img, amount, radius, softness),
        PixelFx::Grain { amount, size, speed, seed } => {
            // new grain `speed` times a second; 0 = still
            let tick = (t * speed as f64).floor() as u32;
            grain(img, amount, (size * d).max(1.0), tick.wrapping_add((seed * 7919.0) as u32))
        }
        PixelFx::Glow { radius, amount, threshold } => glow(img, radius * d, amount, threshold),
        PixelFx::Shadow { x, y, blur, color, opacity } => shadow(img, x * d, y * d, blur * d, color, opacity),
        PixelFx::ChromaKey { color, similarity, smoothness, spill } => chroma_key(img, color, similarity, smoothness, spill),
        PixelFx::Rounded(r) => rounded(img, r * d),
        PixelFx::Crop { left, top, right, bottom } => crop(img, left * d, top * d, right * d, bottom * d),
        PixelFx::Shader { .. } => unreachable!("shaders run in chains"),
    }
}

fn run_shaders(img: &mut Img, fx: &[PixelFx], d: f32, time: FxTime, gpu: &mut GpuSlot) -> Result<()> {
    if gpu.is_none() {
        let g = Gpu::new()?;
        eprintln!("shaders run on {}", g.adapter);
        *gpu = Some(g);
    }
    let passes: Vec<Pass> = fx
        .iter()
        .map(|f| {
            let PixelFx::Shader { program, params, margin } = f else { unreachable!() };
            Pass {
                program,
                params,
                uniforms: Uniforms {
                    time: time.layer as f32,
                    global_time: time.global as f32,
                    frame: (time.global * time.fps).round() as f32,
                    density: d,
                    resolution: [img.w as f32, img.h as f32],
                    size: [img.content.w / d, img.content.h / d],
                    margin: margin * d,
                },
            }
        })
        .collect();
    let mut bytes = to_rgba8(img);
    gpu.as_mut().unwrap().run(&passes, &mut bytes, img.w as u32, img.h as u32)?;
    from_rgba8(img, &bytes);
    Ok(())
}

fn to_rgba8(img: &Img) -> Vec<u8> {
    let mut out = vec![0u8; img.w * img.h * 4];
    out.par_chunks_mut(4).zip(img.px.par_iter()).for_each(|(d, p)| {
        let a = p[3].clamp(0.0, 1.0);
        for i in 0..3 {
            d[i] = (p[i].clamp(0.0, a) * 255.0 + 0.5) as u8;
        }
        d[3] = (a * 255.0 + 0.5) as u8;
    });
    out
}

fn from_rgba8(img: &mut Img, bytes: &[u8]) {
    img.px.par_iter_mut().zip(bytes.par_chunks(4)).for_each(|(p, b)| {
        *p = [b[0] as f32 / 255.0, b[1] as f32 / 255.0, b[2] as f32 / 255.0, b[3] as f32 / 255.0];
    });
}

// ---------- color ----------

fn luma(c: [f32; 3]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn mul3(m: &[[f32; 3]; 3], c: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| m[i][0] * c[0] + m[i][1] * c[1] + m[i][2] * c[2])
}

/// Same matrix as CSS hue-rotate(): keeps luminance.
fn hue_matrix(deg: f32) -> [[f32; 3]; 3] {
    let (s, c) = deg.to_radians().sin_cos();
    [
        [0.213 + c * 0.787 - s * 0.213, 0.715 - c * 0.715 - s * 0.715, 0.072 - c * 0.072 + s * 0.928],
        [0.213 - c * 0.213 + s * 0.143, 0.715 + c * 0.285 + s * 0.140, 0.072 - c * 0.072 - s * 0.283],
        [0.213 - c * 0.213 - s * 0.787, 0.715 - c * 0.715 + s * 0.715, 0.072 + c * 0.928 + s * 0.072],
    ]
}

/// Applies `f` to straight (un-premultiplied) RGB of every visible pixel.
fn map_rgb(img: &mut Img, f: impl Fn([f32; 3]) -> [f32; 3] + Sync) {
    img.px.par_iter_mut().for_each(|p| {
        let a = p[3];
        if a <= 0.0 {
            return;
        }
        let c = f([p[0] / a, p[1] / a, p[2] / a]);
        p[0] = c[0].clamp(0.0, 1.0) * a;
        p[1] = c[1].clamp(0.0, 1.0) * a;
        p[2] = c[2].clamp(0.0, 1.0) * a;
    });
}

// ---------- blur ----------

/// Three box blurs approximate a gaussian. `radius` is roughly how far the
/// blur visibly spreads (sigma = radius / 2).
fn box_radii(radius: f32) -> [usize; 3] {
    let sigma = radius / 2.0;
    if sigma < 0.25 {
        return [0; 3];
    }
    let n = 3.0;
    let w_ideal = (12.0 * sigma * sigma / n + 1.0).sqrt();
    let mut wl = w_ideal.floor() as i64;
    if wl % 2 == 0 {
        wl -= 1;
    }
    let wu = wl + 2;
    let wlf = wl as f32;
    let m = ((12.0 * sigma * sigma - n * wlf * wlf - 4.0 * n * wlf - 3.0 * n) / (-4.0 * wlf - 4.0)).round() as i64;
    [0, 1, 2].map(|i| (((if i < m { wl } else { wu }) - 1) / 2).max(0) as usize)
}

fn blur_extent(radius: f32) -> f32 {
    box_radii(radius).iter().sum::<usize>() as f32
}

fn gaussian(img: &mut Img, radius: f32) {
    let radii = box_radii(radius);
    if radii == [0; 3] {
        return;
    }
    blur_buf(&mut img.px, img.w, img.h, radii);
}

fn blur_buf(px: &mut Vec<Px>, w: usize, h: usize, radii: [usize; 3]) {
    let mut tmp = vec![[0.0f32; 4]; w * h];
    for r in radii {
        box_h(px, &mut tmp, w, r);
        std::mem::swap(px, &mut tmp);
    }
    transpose(px, &mut tmp, w, h);
    for r in radii {
        box_h(&tmp, px, h, r);
        std::mem::swap(px, &mut tmp);
    }
    transpose(&tmp, px, h, w);
}

/// Horizontal box blur of radius r; outside the image counts as transparent.
fn box_h(src: &[Px], dst: &mut [Px], w: usize, r: usize) {
    if r == 0 {
        dst.copy_from_slice(src);
        return;
    }
    let norm = 1.0 / (2 * r + 1) as f32;
    dst.par_chunks_mut(w).zip(src.par_chunks(w)).for_each(|(out, row)| {
        let mut acc = [0.0f32; 4];
        for p in row.iter().take(r.min(w)) {
            add(&mut acc, p, 1.0);
        }
        for x in 0..w {
            if x + r < w {
                add(&mut acc, &row[x + r], 1.0);
            }
            if x > r {
                add(&mut acc, &row[x - r - 1], -1.0);
            }
            out[x] = acc.map(|v| v * norm);
        }
    });
}

fn add(acc: &mut Px, p: &Px, s: f32) {
    for i in 0..4 {
        acc[i] += p[i] * s;
    }
}

fn transpose(src: &[Px], dst: &mut [Px], w: usize, h: usize) {
    // dst is h wide and w tall
    dst.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        for (y, d) in col.iter_mut().enumerate() {
            *d = src[y * w + x];
        }
    });
}

fn sharpen(img: &mut Img, amount: f32, radius: f32) {
    let radii = box_radii(radius);
    if radii == [0; 3] || amount == 0.0 {
        return;
    }
    let mut blurred = img.px.clone();
    blur_buf(&mut blurred, img.w, img.h, radii);
    img.px.par_iter_mut().zip(blurred.par_iter()).for_each(|(p, b)| {
        for i in 0..3 {
            p[i] = (p[i] + (p[i] - b[i]) * amount).clamp(0.0, p[3]);
        }
    });
}

fn pixelate(img: &mut Img, size: f32) {
    let s = size.round() as usize;
    if s <= 1 {
        return;
    }
    let (w, h) = (img.w, img.h);
    // blocks line up with the layer's content, not the margin
    let ox = img.content.x.round() as usize % s;
    let oy = img.content.y.round() as usize % s;
    let starts = |len: usize, off: usize| {
        let mut v = vec![0];
        let mut p = if off == 0 { s } else { off };
        while p < len {
            v.push(p);
            p += s;
        }
        v
    };
    let xs = starts(w, ox);
    let ys = starts(h, oy);
    let px = &mut img.px;
    for (yi, &y0) in ys.iter().enumerate() {
        let y1 = ys.get(yi + 1).copied().unwrap_or(h);
        for (xi, &x0) in xs.iter().enumerate() {
            let x1 = xs.get(xi + 1).copied().unwrap_or(w);
            let mut acc = [0.0f32; 4];
            for y in y0..y1 {
                for p in &px[y * w + x0..y * w + x1] {
                    add(&mut acc, p, 1.0);
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as f32;
            let avg = acc.map(|v| v / n);
            for y in y0..y1 {
                px[y * w + x0..y * w + x1].fill(avg);
            }
        }
    }
}

// ---------- looks ----------

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    if e1 <= e0 {
        return if x < e0 { 0.0 } else { 1.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn vignette(img: &mut Img, amount: f32, radius: f32, softness: f32) {
    let c = img.content;
    let (cx, cy) = (c.x + c.w / 2.0, c.y + c.h / 2.0);
    let w = img.w;
    img.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let dy = ((y as f32 + 0.5 - cy) / (c.h / 2.0)).abs();
        for (x, p) in row.iter_mut().enumerate() {
            let dx = ((x as f32 + 0.5 - cx) / (c.w / 2.0)).abs();
            // 0 in the center, 1 in the corners
            let dist = (dx * dx + dy * dy).sqrt() / std::f32::consts::SQRT_2;
            let k = 1.0 - amount * smoothstep(radius, radius + softness, dist);
            for v in p.iter_mut().take(3) {
                *v *= k;
            }
        }
    });
}

fn hash(x: u32, y: u32, z: u32) -> f32 {
    let mut h = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ z.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h as f32 / u32::MAX as f32) * 2.0 - 1.0
}

/// Monochrome noise in cells of `cell` pixels.
fn grain(img: &mut Img, amount: f32, cell: f32, tick: u32) {
    let w = img.w;
    let (ox, oy) = (img.content.x, img.content.y);
    img.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let gy = ((y as f32 - oy) / cell).floor() as i32 as u32;
        for (x, p) in row.iter_mut().enumerate() {
            if p[3] <= 0.0 {
                continue;
            }
            let gx = ((x as f32 - ox) / cell).floor() as i32 as u32;
            let n = hash(gx, gy, tick) * amount * p[3];
            for v in p.iter_mut().take(3) {
                *v += n;
            }
        }
    });
}

fn glow(img: &mut Img, radius: f32, amount: f32, threshold: f32) {
    let mut bright: Vec<Px> = img
        .px
        .par_iter()
        .map(|p| {
            if p[3] <= 0.0 {
                return [0.0; 4];
            }
            let c = [p[0] / p[3], p[1] / p[3], p[2] / p[3]];
            let k = ((luma(c) - threshold) / (1.0 - threshold)).clamp(0.0, 1.0);
            [p[0] * k, p[1] * k, p[2] * k, p[3] * k]
        })
        .collect();
    blur_buf(&mut bright, img.w, img.h, box_radii(radius));
    img.px.par_iter_mut().zip(bright.par_iter()).for_each(|(p, g)| {
        // additive light
        for i in 0..3 {
            p[i] += g[i] * amount;
        }
        p[3] = (p[3] + g[3] * amount * (1.0 - p[3])).min(1.0);
        p[3] = p[3].max(p[0]).max(p[1]).max(p[2]).min(1.0);
    });
}

fn shadow(img: &mut Img, dx: f32, dy: f32, blur: f32, color: Rgba, opacity: f32) {
    let (w, h) = (img.w, img.h);
    let (ox, oy) = (dx.round() as isize, dy.round() as isize);
    let k = opacity * color[3];
    let mut sh = vec![[0.0f32; 4]; w * h];
    sh.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let sy = y as isize - oy;
        if sy < 0 || sy >= h as isize {
            return;
        }
        for (x, s) in row.iter_mut().enumerate() {
            let sx = x as isize - ox;
            if sx < 0 || sx >= w as isize {
                continue;
            }
            let a = img.px[sy as usize * w + sx as usize][3] * k;
            *s = [color[0] * a, color[1] * a, color[2] * a, a];
        }
    });
    blur_buf(&mut sh, w, h, box_radii(blur));
    // the layer goes over its shadow
    img.px.par_iter_mut().zip(sh.par_iter()).for_each(|(p, s)| {
        let inv = 1.0 - p[3];
        for i in 0..4 {
            p[i] += s[i] * inv;
        }
    });
}

// ---------- compositing ----------

fn chroma(c: [f32; 3]) -> (f32, f32) {
    (
        -0.1687 * c[0] - 0.3313 * c[1] + 0.5 * c[2],
        0.5 * c[0] - 0.4187 * c[1] - 0.0813 * c[2],
    )
}

fn chroma_key(img: &mut Img, key: Rgba, similarity: f32, smoothness: f32, spill: f32) {
    let (kb, kr) = chroma([key[0], key[1], key[2]]);
    // the channel the key is made of (green for a green screen)
    let dom = (0..3).max_by(|&a, &b| key[a].total_cmp(&key[b])).unwrap();
    img.px.par_iter_mut().for_each(|p| {
        let a = p[3];
        if a <= 0.0 {
            return;
        }
        let mut c = [p[0] / a, p[1] / a, p[2] / a];
        let (cb, cr) = chroma(c);
        let dist = ((cb - kb).powi(2) + (cr - kr).powi(2)).sqrt();
        let keep = smoothstep(similarity, similarity + smoothness, dist);
        if spill > 0.0 {
            // pull the key color out of edges and reflections
            let others: Vec<f32> = (0..3).filter(|&i| i != dom).map(|i| c[i]).collect();
            let limit = (others[0] + others[1]) / 2.0;
            if c[dom] > limit {
                c[dom] -= (c[dom] - limit) * spill;
            }
        }
        let na = a * keep;
        *p = [c[0] * na, c[1] * na, c[2] * na, na];
    });
}

/// Multiplies every pixel by coverage(x, y) in 0..1.
fn mask(img: &mut Img, coverage: impl Fn(f32, f32) -> f32 + Sync) {
    let w = img.w;
    img.px.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        for (x, p) in row.iter_mut().enumerate() {
            let k = coverage(x as f32 + 0.5, y as f32 + 0.5);
            if k < 1.0 {
                *p = p.map(|v| v * k);
            }
        }
    });
}

fn rounded(img: &mut Img, r: f32) {
    let c = img.content;
    let r = r.min(c.w / 2.0).min(c.h / 2.0);
    if r <= 0.0 {
        return;
    }
    let (cx, cy) = (c.x + c.w / 2.0, c.y + c.h / 2.0);
    let (hw, hh) = (c.w / 2.0 - r, c.h / 2.0 - r);
    mask(img, |x, y| {
        // signed distance to a rounded rectangle
        let qx = (x - cx).abs() - hw;
        let qy = (y - cy).abs() - hh;
        let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
        let inside = qx.max(qy).min(0.0);
        (0.5 - (outside + inside - r)).clamp(0.0, 1.0)
    });
}

fn crop(img: &mut Img, left: f32, top: f32, right: f32, bottom: f32) {
    let c = img.content;
    let (x0, x1) = (c.x + left, c.x + c.w - right);
    let (y0, y1) = (c.y + top, c.y + c.h - bottom);
    mask(img, |x, y| {
        let kx = (x - x0 + 0.5).min(x1 - x + 0.5).clamp(0.0, 1.0);
        let ky = (y - y0 + 0.5).min(y1 - y + 0.5).clamp(0.0, 1.0);
        kx * ky
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: FxTime = FxTime { layer: 0.0, global: 0.0, fps: 30.0 };

    fn solid(w: u32, h: u32, rgba: [u8; 4]) -> Pixmap {
        let mut p = Pixmap::new(w, h).unwrap();
        for px in p.data_mut().chunks_exact_mut(4) {
            px.copy_from_slice(&rgba);
        }
        p
    }

    fn full(p: &Pixmap) -> ContentRect {
        ContentRect { x: 0.0, y: 0.0, w: p.width() as f32, h: p.height() as f32 }
    }

    #[test]
    fn blur_grows_and_keeps_total_alpha() {
        let src = solid(20, 20, [255, 255, 255, 255]);
        let out = apply(&src, full(&src), &[PixelFx::Blur(6.0)], 1.0, T0, &mut None).unwrap();
        assert!(out.grow > 0.0);
        let total: f64 = out.pixmap.data().chunks_exact(4).map(|p| p[3] as f64).sum();
        let expected = 20.0 * 20.0 * 255.0;
        assert!((total - expected).abs() / expected < 0.02, "{total} vs {expected}");
    }

    #[test]
    fn grayscale_makes_channels_equal() {
        let src = solid(4, 4, [200, 50, 10, 255]);
        let out = apply(&src, full(&src), &[PixelFx::Grayscale(1.0)], 1.0, T0, &mut None).unwrap();
        let p = &out.pixmap.data()[0..4];
        assert_eq!(p[0], p[1]);
        assert_eq!(p[1], p[2]);
    }

    #[test]
    fn chroma_key_removes_green_only() {
        let green = solid(2, 2, [0, 255, 0, 255]);
        let out = apply(&green, full(&green), &[PixelFx::ChromaKey { color: [0.0, 1.0, 0.0, 1.0], similarity: 0.25, smoothness: 0.08, spill: 0.5 }], 1.0, T0, &mut None).unwrap();
        assert_eq!(out.pixmap.data()[3], 0);
        let red = solid(2, 2, [255, 0, 0, 255]);
        let out = apply(&red, full(&red), &[PixelFx::ChromaKey { color: [0.0, 1.0, 0.0, 1.0], similarity: 0.25, smoothness: 0.08, spill: 0.5 }], 1.0, T0, &mut None).unwrap();
        assert_eq!(out.pixmap.data()[3], 255);
    }

    #[test]
    fn rounded_clears_corners() {
        let src = solid(100, 100, [255, 255, 255, 255]);
        let out = apply(&src, full(&src), &[PixelFx::Rounded(30.0)], 1.0, T0, &mut None).unwrap();
        let d = out.pixmap.data();
        assert_eq!(d[3], 0); // top-left corner
        assert_eq!(d[(50 * 100 + 50) * 4 + 3], 255); // center
    }

    #[test]
    fn density_scales_lengths() {
        assert!(blur_extent(10.0 * 0.5) < blur_extent(10.0));
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use std::time::Instant;

    /// cargo test --release -- --ignored --nocapture pixel_bench
    #[test]
    #[ignore]
    fn pixel_bench() {
        let mut src = Pixmap::new(1920, 1080).unwrap();
        for (i, px) in src.data_mut().chunks_exact_mut(4).enumerate() {
            px.copy_from_slice(&[(i % 251) as u8, (i % 241) as u8, (i % 239) as u8, 255]);
        }
        let rect = ContentRect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 };
        let cases: Vec<(&str, Vec<PixelFx>)> = vec![
            ("convert only", vec![]),
            ("saturation", vec![PixelFx::Saturation(1.2)]),
            ("vignette", vec![PixelFx::Vignette { amount: 0.5, radius: 0.5, softness: 0.5 }]),
            ("grain", vec![PixelFx::Grain { amount: 0.05, size: 1.5, speed: 12.0, seed: 0.0 }]),
            ("blur 6", vec![PixelFx::Blur(6.0)]),
            ("blur 40", vec![PixelFx::Blur(40.0)]),
            ("glow", vec![PixelFx::Glow { radius: 20.0, amount: 0.8, threshold: 0.6 }]),
        ];
        for (name, fx) in cases {
            let n = 10;
            let t = Instant::now();
            for f in 0..n {
                apply(&src, rect, &fx, 1.0, FxTime { layer: f as f64, global: f as f64, fps: 30.0 }, &mut None).unwrap();
            }
            eprintln!("{name:<14} {:>6.1} ms", t.elapsed().as_secs_f64() * 1000.0 / n as f64);
        }
    }
}
