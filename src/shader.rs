//! Shader effects: user WGSL wrapped into a full program, parameters packed
//! into a uniform buffer, and validation with readable errors. No GPU is
//! needed here, so `clve check` can validate shaders on any machine.

use crate::fx::Val;
use crate::scene::parse_color;
use anyhow::{anyhow, bail, Result};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex, OnceLock};

/// Layout of the per-effect parameter struct `p`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Kind {
    Float,
    Color,
}

/// A validated shader program, shared by every frame that uses it.
#[derive(Debug)]
pub struct Program {
    pub name: String,
    pub wgsl: String,
    pub hash: u64,
    /// Parameter names in buffer order.
    pub layout: Vec<(String, Kind)>,
}

impl PartialEq for Program {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
    }
}

impl Program {
    /// Packs parameter values into the uniform buffer layout of `p`.
    pub fn pack(&self, vals: &HashMap<String, Val>) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        for (key, kind) in &self.layout {
            match (kind, vals.get(key)) {
                (Kind::Color, Some(Val::Str(s))) => {
                    let c = parse_color(s).map_err(|e| anyhow!("effect '{}': {key}: {e}", self.name))?;
                    for v in c {
                        out.extend_from_slice(&(v as f32 / 255.0).to_le_bytes());
                    }
                }
                (Kind::Float, Some(Val::Num(n))) => out.extend_from_slice(&(*n as f32).to_le_bytes()),
                (_, None) => bail!("effect '{}': missing parameter '{key}'", self.name),
                (Kind::Color, _) => bail!("effect '{}': {key} must be a color like \"#ff8800\"", self.name),
                (Kind::Float, _) => bail!("effect '{}': {key} must be a number", self.name),
            }
        }
        // WGSL uniform structs are at least 16 bytes and a multiple of 16
        let size = out.len().max(16).div_ceil(16) * 16;
        out.resize(size, 0);
        Ok(out)
    }
}

/// Values every shader gets as `u`.
pub struct Uniforms {
    /// Seconds since the layer started.
    pub time: f32,
    /// Seconds on the whole timeline.
    pub global_time: f32,
    pub frame: f32,
    /// Image pixels per project pixel.
    pub density: f32,
    /// Size of the image the shader draws, in pixels (margin included).
    pub resolution: [f32; 2],
    /// Size of the layer itself, in project pixels.
    pub size: [f32; 2],
    /// Margin on each side, in image pixels.
    pub margin: f32,
}

impl Uniforms {
    pub fn bytes(&self) -> Vec<u8> {
        let vals = [
            self.time,
            self.global_time,
            self.frame,
            self.density,
            self.resolution[0],
            self.resolution[1],
            self.size[0],
            self.size[1],
            self.margin,
            0.0,
            0.0,
            0.0,
        ];
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }
}

const HEADER: &str = r#"// ---- clve shader prelude ----
struct Clve {
    time: f32,
    global_time: f32,
    frame: f32,
    density: f32,
    resolution: vec2f,
    size: vec2f,
    margin: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}
@group(0) @binding(0) var<uniform> u: Clve;
@group(0) @binding(2) var clve_tex: texture_2d<f32>;
@group(0) @binding(3) var clve_samp: sampler;

const PI: f32 = 3.14159265358979;

// The layer's color at uv (0..1 across the image), straight alpha.
// Outside the image it is transparent.
fn sample(uv: vec2f) -> vec4f {
    if (any(uv < vec2f(0.0)) || any(uv > vec2f(1.0))) {
        return vec4f(0.0);
    }
    let c = textureSampleLevel(clve_tex, clve_samp, uv, 0.0);
    if (c.a <= 0.0) {
        return vec4f(0.0);
    }
    return vec4f(c.rgb / c.a, c.a);
}

// Same as sample(), in pixels of the image.
fn pixel(xy: vec2f) -> vec4f {
    return sample(xy / u.resolution);
}

fn luma(c: vec3f) -> f32 {
    return dot(c, vec3f(0.2126, 0.7152, 0.0722));
}

// 0..1, stable for the same input.
fn hash(p: vec2f) -> f32 {
    let h = dot(p, vec2f(127.1, 311.7));
    return fract(sin(h) * 43758.5453);
}

// Smooth noise, -1..1.
fn noise(p: vec2f) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let s = f * f * (3.0 - 2.0 * f);
    let a = hash(i);
    let b = hash(i + vec2f(1.0, 0.0));
    let c = hash(i + vec2f(0.0, 1.0));
    let d = hash(i + vec2f(1.0, 1.0));
    return mix(mix(a, b, s.x), mix(c, d, s.x), s.y) * 2.0 - 1.0;
}

fn rotate(v: vec2f, degrees: f32) -> vec2f {
    let a = radians(degrees);
    let cs = cos(a);
    let sn = sin(a);
    return vec2f(v.x * cs - v.y * sn, v.x * sn + v.y * cs);
}
"#;

const FOOTER: &str = r#"
// ---- clve entry points ----
@vertex
fn clve_vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
    let q = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4f(q * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn clve_fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    _ = p;
    let c = clamp(effect(pos.xy / u.resolution), vec4f(0.0), vec4f(1.0));
    return vec4f(c.rgb * c.a, c.a);
}
"#;

/// Where the user's shader code came from, for error messages.
pub struct Origin<'a> {
    /// e.g. "effects/wave.lua"
    pub file: &'a str,
    /// Full text of that file, to find the line the shader starts on.
    pub file_text: Option<&'a str>,
}

fn cache() -> &'static Mutex<HashMap<u64, Arc<Program>>> {
    static CACHE: OnceLock<Mutex<HashMap<u64, Arc<Program>>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// Builds and validates the program for a shader effect. Cached, so calling
/// it every frame costs a hash lookup.
/// `defaults` (the effect's `params` table) decides each parameter's type.
pub fn program(name: &str, user: &str, defaults: &HashMap<String, Val>, origin: &Origin) -> Result<Arc<Program>> {
    // colors first, then floats: no padding needed between them
    let mut layout: Vec<(String, Kind)> = defaults
        .iter()
        .map(|(k, v)| {
            let kind = match v {
                Val::Num(_) => Kind::Float,
                Val::Str(_) => Kind::Color,
            };
            (k.clone(), kind)
        })
        .collect();
    layout.sort_by(|a, b| (a.1 == Kind::Float, &a.0).cmp(&(b.1 == Kind::Float, &b.0)));
    for (k, _) in &layout {
        let ok = k.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !ok {
            bail!("effect '{name}': parameter '{k}' can't be used in a shader (letters, digits and _ only)");
        }
    }

    let mut h = std::collections::hash_map::DefaultHasher::new();
    (name, user, &layout).hash(&mut h);
    let key = h.finish();
    if let Some(p) = cache().lock().unwrap().get(&key) {
        return Ok(p.clone());
    }

    let mut params_struct = String::from("struct Params {\n");
    for (k, kind) in &layout {
        let ty = match kind {
            Kind::Color => "vec4f",
            Kind::Float => "f32",
        };
        params_struct += &format!("    {k}: {ty},\n");
    }
    if layout.is_empty() {
        params_struct += "    _unused: f32,\n";
    }
    params_struct += "}\n@group(0) @binding(1) var<uniform> p: Params;\n";

    let head = format!("{HEADER}{params_struct}// ---- effect ----\n");
    let head_lines = head.lines().count();
    let wgsl = format!("{head}{user}\n{FOOTER}");

    validate(&wgsl, head_lines, user, origin)?;

    let prog = Arc::new(Program {
        name: name.to_string(),
        wgsl,
        hash: key,
        layout,
    });
    cache().lock().unwrap().insert(key, prog.clone());
    Ok(prog)
}

fn validate(wgsl: &str, head_lines: usize, user: &str, origin: &Origin) -> Result<()> {
    // checked on the text: without it the error would point at clve's own code
    let defines_effect = user
        .split("fn")
        .skip(1)
        .any(|rest| rest.trim_start().starts_with("effect") && rest.trim_start()[6..].trim_start().starts_with('('));
    if !defines_effect {
        bail!("{}: the shader must define fn effect(uv: vec2f) -> vec4f", origin.file);
    }
    let located = |line: Option<u32>, msg: String| -> anyhow::Error {
        anyhow!("{}", locate(line, head_lines, user, origin, &msg))
    };

    let module = match naga::front::wgsl::parse_str(wgsl) {
        Ok(m) => m,
        Err(e) => {
            let line = e.location(wgsl).map(|l| l.line_number);
            return Err(located(line, e.message().to_string()));
        }
    };
    let mut v = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default());
    if let Err(e) = v.validate(&module) {
        // the most specific span points at the offending expression
        let line = e
            .spans()
            .last()
            .map(|(span, _)| span.location(wgsl).line_number)
            .or_else(|| e.location(wgsl).map(|l| l.line_number));
        // the last error in the chain says what is actually wrong
        let mut msg = e.to_string();
        let mut src = std::error::Error::source(&e);
        while let Some(s) = src {
            msg = s.to_string();
            src = s.source();
        }
        return Err(located(line, pretty(&msg)));
    }
    Ok(())
}

/// Makes naga's messages readable: "Vector { size: Quad, scalar: Scalar { kind: Float, width: 4 } }" → "vec4f".
fn pretty(msg: &str) -> String {
    let mut out = msg.to_string();
    for (size, n) in [("Bi", 2), ("Tri", 3), ("Quad", 4)] {
        for (kind, suffix) in [("Float", "f"), ("Sint", "i"), ("Uint", "u"), ("Bool", "<bool>")] {
            let from = format!("Vector {{ size: {size}, scalar: Scalar {{ kind: {kind}, width: 4 }} }}");
            let to = if kind == "Bool" { format!("vec{n}<bool>") } else { format!("vec{n}{suffix}") };
            out = out.replace(&from, &to);
        }
    }
    for (kind, name) in [("Float", "f32"), ("Sint", "i32"), ("Uint", "u32")] {
        out = out.replace(&format!("Scalar(Scalar {{ kind: {kind}, width: 4 }})"), name);
        out = out.replace(&format!("Scalar {{ kind: {kind}, width: 4 }}"), name);
    }
    out = out.replace("Scalar(Scalar { kind: Bool, width: 1 })", "bool");
    // "[3] (of type vec4f)" → "vec4f"
    let mut clean = String::new();
    let mut rest = out.as_str();
    while let Some(i) = rest.find('[') {
        let (before, after) = rest.split_at(i);
        clean += before;
        match after.find("] (of type ") {
            Some(j) if after[1..j].chars().all(|c| c.is_ascii_digit()) => {
                let tail = &after[j + "] (of type ".len()..];
                match tail.find(')') {
                    Some(k) => {
                        clean += &tail[..k];
                        rest = &tail[k + 1..];
                    }
                    None => {
                        clean += after;
                        rest = "";
                    }
                }
            }
            _ => {
                clean.push('[');
                rest = &after[1..];
            }
        }
    }
    clean += rest;
    clean.replace("Operation Multiply", "can't multiply:").replace("can't multiply: can't work with", "can't multiply")
        .replace("Operation Add can't work with", "can't add")
        .replace("Operation Subtract can't work with", "can't subtract")
        .replace("Operation Divide can't work with", "can't divide")
}

/// Turns a line of the composed program into "effects/x.lua:12".
fn locate(line: Option<u32>, head_lines: usize, user: &str, origin: &Origin, msg: &str) -> String {
    let Some(line) = line else {
        return format!("{}: shader: {msg}", origin.file);
    };
    let line = line as usize;
    let user_lines = user.lines().count().max(1);
    if line <= head_lines || line > head_lines + user_lines {
        return format!("{}: shader: {msg} (in clve's code around your shader — maybe a name clash with a built-in helper?)", origin.file);
    }
    let in_user = line - head_lines; // 1-based line inside the shader string
    let start = origin
        .file_text
        .and_then(|text| text.find(user).map(|pos| text[..pos].matches('\n').count() + 1));
    match start {
        Some(first) => format!("{}:{}: {msg}", origin.file, first + in_user - 1),
        None => format!("{}: shader line {in_user}: {msg}", origin.file),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin<'a>(text: &'a str) -> Origin<'a> {
        Origin { file: "effects/t.lua", file_text: Some(text) }
    }

    #[test]
    fn valid_shader_compiles() {
        let user = "fn effect(uv: vec2f) -> vec4f {\n  return sample(uv) * p.amount + p.tint;\n}";
        let mut params = HashMap::new();
        params.insert("amount".to_string(), Val::Num(0.5));
        params.insert("tint".to_string(), Val::Str("#ff0000".into()));
        let prog = program("t", user, &params, &origin(user)).unwrap();
        assert_eq!(prog.layout[0].0, "tint"); // colors first
        let bytes = prog.pack(&params).unwrap();
        assert_eq!(bytes.len(), 32);
    }

    #[test]
    fn errors_point_at_the_lua_file_line() {
        let file = "params = {}\n\nshader = [[\nfn effect(uv: vec2f) -> vec4f {\n  return sample(uv) + undefined_thing;\n}\n]]\n";
        let user = "fn effect(uv: vec2f) -> vec4f {\n  return sample(uv) + undefined_thing;\n}\n";
        let err = program("t", user, &HashMap::new(), &origin(file)).unwrap_err().to_string();
        assert!(err.starts_with("effects/t.lua:5:"), "{err}");
    }

    #[test]
    fn type_errors_are_readable() {
        let user = "fn effect(uv: vec2f) -> vec4f {\n  let c = sample(uv);\n  return c * vec3f(1.0);\n}\n";
        let err = program("t", user, &HashMap::new(), &origin(user)).unwrap_err().to_string();
        assert!(err.contains("effects/t.lua:3:"), "{err}");
        assert!(err.contains("vec4f") && err.contains("vec3f") && !err.contains("Quad"), "{err}");
    }

    #[test]
    fn missing_effect_function() {
        let user = "fn other(uv: vec2f) -> vec4f { return vec4f(0.0); }";
        let err = program("t", user, &HashMap::new(), &origin(user)).unwrap_err().to_string();
        assert!(err.contains("fn effect"), "{err}");
    }
}
