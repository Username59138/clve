//! Lua scripting: every layer gets its own sandboxed Lua state running
//! layers/<name>/layer.lua. `frame(t, layer)` is called for every frame with a
//! fresh copy of the layer's properties from layer.toml.

use crate::check::Span;
use crate::fx::{self, Def, Val};
use crate::project::EFFECTS_DIR;
use crate::scene::{parse_anchor, parse_color, Align, LayerState};
use anyhow::{anyhow, Result};
use mlua::{Function, HookTriggers, Lua, LuaOptions, StdLib, Table, Value, VmState};
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

const PRELUDE: &str = include_str!("lua/prelude.lua");
const MEMORY_LIMIT: usize = 256 * 1024 * 1024;
const FRAME_TIME_LIMIT: Duration = Duration::from_secs(2);

/// Built-in effects, shipped inside the binary.
pub const BUILTIN_EFFECTS: &[(&str, &str)] = &[
    ("fade", include_str!("lua/effects/fade.lua")),
    ("pop", include_str!("lua/effects/pop.lua")),
    ("pulse", include_str!("lua/effects/pulse.lua")),
    ("shake", include_str!("lua/effects/shake.lua")),
    ("slide_in", include_str!("lua/effects/slide_in.lua")),
    ("spin", include_str!("lua/effects/spin.lua")),
    ("zoom", include_str!("lua/effects/zoom.lua")),
];

/// Project-wide values visible to scripts as `project.*`.
#[derive(Clone)]
pub struct Env {
    pub root: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: f64,
}

pub struct Script {
    root: PathBuf,
    lua: Lua,
    frame: Function,
    methods: Table,
    clip: Table,
    deadline: Rc<Cell<Option<Instant>>>,
}

impl Script {
    /// Loads the layer's script. None if there is no layer.lua or it defines no `frame`.
    pub fn load(span: &Span, env: &Env) -> Result<Option<Script>> {
        let path = span.dir.join("layer.lua");
        if !path.is_file() {
            return Ok(None);
        }
        let src = std::fs::read_to_string(&path)?;
        let chunk_name = format!("@layers/{}/layer.lua", span.name);

        let lua = Lua::new_with(
            StdLib::MATH | StdLib::STRING | StdLib::TABLE | StdLib::UTF8,
            LuaOptions::default(),
        )
        .map_err(lua_err)?;
        lua.set_memory_limit(MEMORY_LIMIT).map_err(lua_err)?;

        // stop runaway scripts (e.g. an infinite loop) instead of hanging the render
        let deadline: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
        let d = deadline.clone();
        lua.set_hook(HookTriggers::new().every_nth_instruction(10_000), move |_, _| {
            match d.get() {
                Some(end) if Instant::now() > end => Err(mlua::Error::runtime(format!(
                    "script ran longer than {}s on one frame (infinite loop?)",
                    FRAME_TIME_LIMIT.as_secs()
                ))),
                _ => Ok(VmState::Continue),
            }
        })
        .map_err(lua_err)?;

        let g = lua.globals();
        let name = span.name.clone();
        let print = lua
            .create_function(move |_, args: mlua::Variadic<Value>| {
                let parts: Vec<String> = args
                    .iter()
                    .map(|v| v.to_string().unwrap_or_else(|_| "?".into()))
                    .collect();
                eprintln!("[{name}] {}", parts.join("\t"));
                Ok(())
            })
            .map_err(lua_err)?;
        g.set("print", print).map_err(lua_err)?;

        let project = lua.create_table().map_err(lua_err)?;
        project.set("width", env.width).map_err(lua_err)?;
        project.set("height", env.height).map_err(lua_err)?;
        project.set("fps", env.fps).map_err(lua_err)?;
        project.set("duration", env.duration).map_err(lua_err)?;
        g.set("project", project).map_err(lua_err)?;

        let clip = lua.create_table().map_err(lua_err)?;
        clip.set("name", span.name.as_str()).map_err(lua_err)?;
        clip.set("type", span.kind.to_string()).map_err(lua_err)?;
        clip.set("start", span.start).map_err(lua_err)?;
        clip.set("duration", span.end - span.start).map_err(lua_err)?;
        clip.set("global_t", span.start).map_err(lua_err)?;
        g.set("clip", clip.clone()).map_err(lua_err)?;

        let root = env.root.clone();
        let effect_source = lua
            .create_function(move |_, name: String| {
                effect_source(&root, &name).map_err(mlua::Error::runtime)
            })
            .map_err(lua_err)?;
        g.set("__clve_effect_source", effect_source).map_err(lua_err)?;

        let root = env.root.clone();
        let has_project_effect = lua
            .create_function(move |_, name: String| {
                let ok = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-');
                Ok(ok && root.join(EFFECTS_DIR).join(format!("{name}.lua")).is_file())
            })
            .map_err(lua_err)?;
        g.set("__clve_has_project_effect", has_project_effect).map_err(lua_err)?;

        // built-in pixel effects and their defaults
        let pixel = lua.create_table().map_err(lua_err)?;
        for spec in fx::SPECS {
            let defaults = lua.create_table().map_err(lua_err)?;
            for (key, def) in spec.params {
                match def {
                    Def::Num(n) => defaults.set(*key, *n).map_err(lua_err)?,
                    Def::Color(c) => defaults.set(*key, *c).map_err(lua_err)?,
                }
            }
            pixel.set(spec.name, defaults).map_err(lua_err)?;
        }
        g.set("__clve_pixel_fx", pixel).map_err(lua_err)?;

        lua.load(PRELUDE).set_name("=clve").exec().map_err(lua_err)?;
        // nothing past this point may load code, touch files or dump bytecode
        for k in ["load", "loadfile", "dofile", "require", "collectgarbage"] {
            g.set(k, Value::Nil).map_err(lua_err)?;
        }
        if let Ok(string) = g.get::<Table>("string") {
            string.set("dump", Value::Nil).map_err(lua_err)?;
        }
        let methods: Table = g.get("__clve_layer_methods").map_err(lua_err)?;

        deadline.set(Some(Instant::now() + FRAME_TIME_LIMIT));
        let run = lua.load(&src).set_name(chunk_name).exec();
        deadline.set(None);
        run.map_err(lua_err)?;

        let frame = match g.get::<Value>("frame").map_err(lua_err)? {
            Value::Function(f) => f,
            Value::Nil => return Ok(None),
            _ => return Err(anyhow!("layers/{}/layer.lua: `frame` must be a function", span.name)),
        };
        Ok(Some(Script {
            root: env.root.clone(),
            lua,
            frame,
            methods,
            clip,
            deadline,
        }))
    }

    /// Runs `frame(t, layer)` and writes the result back into `st`.
    /// `t` is the time since the layer started, `global_t` the timeline time.
    pub fn apply(&self, t: f64, global_t: f64, st: &mut LayerState) -> Result<()> {
        let tbl = self.to_table(st).map_err(lua_err)?;
        self.clip.set("global_t", global_t).map_err(lua_err)?;
        self.lua.globals().set("__clve_t", t).map_err(lua_err)?;

        self.deadline.set(Some(Instant::now() + FRAME_TIME_LIMIT));
        let res = self.frame.call::<()>((t, tbl.clone()));
        self.deadline.set(None);
        res.map_err(lua_err)?;

        read_back(&tbl, st, &self.root)
    }

    fn to_table(&self, st: &LayerState) -> mlua::Result<Table> {
        let t = self.lua.create_table()?;
        t.set("x", st.x)?;
        t.set("y", st.y)?;
        t.set("scale", st.scale)?;
        t.set("rotation", st.rotation)?;
        t.set("opacity", st.opacity)?;
        match anchor_name(st.anchor) {
            Some(name) => t.set("anchor", name)?,
            None => t.set("anchor", vec![st.anchor.0, st.anchor.1])?,
        }
        t.set("visible", st.visible)?;
        t.set("volume", st.volume)?;
        t.set("pan", st.pan)?;
        t.set("text", st.text.as_str())?;
        t.set("content", st.text.as_str())?;
        t.set("font", st.font.as_str())?;
        t.set("size", st.size)?;
        t.set(
            "align",
            match st.align {
                Align::Left => "left",
                Align::Center => "center",
                Align::Right => "right",
            },
        )?;
        let [r, g, b, a] = st.color;
        let color = if a == 255 {
            format!("#{r:02x}{g:02x}{b:02x}")
        } else {
            format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
        };
        t.set("color", color)?;
        let meta = self.lua.create_table()?;
        meta.set("__index", self.methods.clone())?;
        t.set_metatable(Some(meta))?;
        Ok(t)
    }
}

fn read_back(t: &Table, st: &mut LayerState, root: &Path) -> Result<()> {
    let num = |key: &str| -> Result<f64> {
        match t.raw_get::<Value>(key).map_err(lua_err)? {
            Value::Integer(i) => Ok(i as f64),
            Value::Number(n) if n.is_finite() => Ok(n),
            Value::Number(_) => Err(anyhow!("layer.{key} is not a finite number")),
            other => Err(anyhow!("layer.{key} must be a number, got {}", other.type_name())),
        }
    };
    let string = |key: &str| -> Result<String> {
        match t.raw_get::<Value>(key).map_err(lua_err)? {
            Value::String(s) => Ok(s.to_str().map_err(lua_err)?.to_string()),
            Value::Integer(i) => Ok(i.to_string()),
            Value::Number(n) => Ok(n.to_string()),
            other => Err(anyhow!("layer.{key} must be a string, got {}", other.type_name())),
        }
    };

    st.x = num("x")?;
    st.y = num("y")?;
    st.scale = num("scale")?;
    st.rotation = num("rotation")?;
    st.opacity = num("opacity")?.clamp(0.0, 1.0);
    st.volume = num("volume")?.max(0.0);
    st.pan = num("pan")?.clamp(-1.0, 1.0);
    st.size = num("size")?;
    if st.size <= 0.0 {
        return Err(anyhow!("layer.size must be greater than zero"));
    }
    st.visible = match t.raw_get::<Value>("visible").map_err(lua_err)? {
        Value::Boolean(b) => b,
        Value::Nil => false,
        other => return Err(anyhow!("layer.visible must be true or false, got {}", other.type_name())),
    };
    st.text = string("text")?;
    st.font = string("font")?;
    st.color = parse_color(&string("color")?).map_err(|e| anyhow!("layer.{e}"))?;
    st.align = match string("align")?.as_str() {
        "left" => Align::Left,
        "center" => Align::Center,
        "right" => Align::Right,
        other => return Err(anyhow!("layer.align = \"{other}\": expected left, center or right")),
    };
    st.anchor = match t.raw_get::<Value>("anchor").map_err(lua_err)? {
        Value::String(s) => parse_anchor(&s.to_str().map_err(lua_err)?).map_err(|e| anyhow!("layer.{e}"))?,
        // {0.5, 1} — fractions of the layer size
        Value::Table(v) => {
            let ax: f64 = v.get(1).map_err(|_| anyhow!("layer.anchor = {{x, y}} needs two numbers"))?;
            let ay: f64 = v.get(2).map_err(|_| anyhow!("layer.anchor = {{x, y}} needs two numbers"))?;
            (ax, ay)
        }
        other => return Err(anyhow!("layer.anchor must be a name or {{x, y}}, got {}", other.type_name())),
    };
    st.fx = read_fx(t, root)?;
    Ok(())
}

/// A shader effect: compile (cached) and pack its parameters.
fn shader_fx(root: &Path, entry: &Table, name: &str, src: &str, vals: &std::collections::HashMap<String, Val>) -> Result<fx::PixelFx> {
    let margin: f64 = entry.get("margin").map_err(lua_err)?;
    if !(0.0..=2000.0).contains(&margin) {
        return Err(anyhow!("effect '{name}': margin must be between 0 and 2000 pixels"));
    }
    let chunk: String = entry.get("chunk").map_err(lua_err)?;
    // "@effects/wave.lua" is a file in the project; "=effect 'x'" is built in
    let (file, text) = match chunk.strip_prefix('@') {
        Some(rel) => (rel.to_string(), std::fs::read_to_string(root.join(rel)).ok()),
        None => (format!("effect '{name}'"), None),
    };
    let origin = crate::shader::Origin { file: &file, file_text: text.as_deref() };
    // parameter types come from the effect's defaults, not from what was passed
    let defaults_tbl: Table = entry.get("defaults").map_err(lua_err)?;
    let mut defaults = std::collections::HashMap::new();
    for pair in defaults_tbl.pairs::<String, Value>() {
        let (k, v) = pair.map_err(lua_err)?;
        let v = match v {
            Value::Integer(i) => Val::Num(i as f64),
            Value::Number(n) => Val::Num(n),
            Value::Boolean(_) => Val::Num(0.0),
            Value::String(s) => Val::Str(s.to_str().map_err(lua_err)?.to_string()),
            other => return Err(anyhow!("effect '{name}': default for {k} must be a number, a color or true/false, got {}", other.type_name())),
        };
        defaults.insert(k, v);
    }
    let program = crate::shader::program(name, src, &defaults, &origin)?;
    let params = program.pack(vals)?;
    Ok(fx::PixelFx::Shader { program, params, margin: margin as f32 })
}

/// Pixel effects recorded by layer:effect() during this frame.
fn read_fx(t: &Table, root: &Path) -> Result<Vec<fx::PixelFx>> {
    let list = match t.raw_get::<Value>("__fx").map_err(lua_err)? {
        Value::Table(l) => l,
        _ => return Ok(Vec::new()),
    };
    let mut out = Vec::new();
    for entry in list.sequence_values::<Table>() {
        let entry = entry.map_err(lua_err)?;
        let name: String = entry.get("name").map_err(lua_err)?;
        let shader: Option<String> = entry.get("shader").map_err(lua_err)?;
        let params: Table = entry.get("params").map_err(lua_err)?;
        let mut vals = std::collections::HashMap::new();
        for pair in params.pairs::<String, Value>() {
            let (k, v) = pair.map_err(lua_err)?;
            let v = match v {
                Value::Integer(i) => Val::Num(i as f64),
                Value::Number(n) => Val::Num(n),
                Value::String(s) => Val::Str(s.to_str().map_err(lua_err)?.to_string()),
                // handy for switches in shaders: true = 1, false = 0
                Value::Boolean(b) if shader.is_some() => Val::Num(if b { 1.0 } else { 0.0 }),
                other => {
                    return Err(anyhow!(
                        "effect '{name}': {k} must be a number or a string, got {}",
                        other.type_name()
                    ))
                }
            };
            vals.insert(k, v);
        }
        match shader {
            Some(src) => out.push(shader_fx(root, &entry, &name, &src, &vals)?),
            None => out.push(fx::build(&name, &vals)?),
        }
    }
    Ok(out)
}

fn anchor_name(a: (f64, f64)) -> Option<&'static str> {
    Some(match a {
        (0.5, 0.5) => "center",
        (0.0, 0.0) => "top-left",
        (0.5, 0.0) => "top",
        (1.0, 0.0) => "top-right",
        (0.0, 0.5) => "left",
        (1.0, 0.5) => "right",
        (0.0, 1.0) => "bottom-left",
        (0.5, 1.0) => "bottom",
        (1.0, 1.0) => "bottom-right",
        _ => return None,
    })
}

/// Effects are looked up in the project's effects/ first, then among the built-ins,
/// so a project can override a built-in effect by name.
fn effect_source(root: &Path, name: &str) -> Result<(String, String), String> {
    let ok_name = !name.is_empty()
        && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-');
    if !ok_name {
        return Err(format!("bad effect name '{name}'"));
    }
    let path = root.join(EFFECTS_DIR).join(format!("{name}.lua"));
    if path.is_file() {
        let src = std::fs::read_to_string(&path).map_err(|e| format!("effects/{name}.lua: {e}"))?;
        return Ok((src, format!("@effects/{name}.lua")));
    }
    match BUILTIN_EFFECTS.iter().find(|(n, _)| *n == name) {
        Some((_, src)) => Ok((src.to_string(), format!("=effect '{name}'"))),
        None => {
            Err(format!("unknown effect '{name}' (see: clve list effects; or create effects/{name}.lua)"))
        }
    }
}

/// Turns an mlua error into a short message: "layers/x/layer.lua:3: attempt to ..."
pub fn lua_err(e: mlua::Error) -> anyhow::Error {
    fn msg(e: &mlua::Error) -> String {
        match e {
            mlua::Error::RuntimeError(s) => s.clone(),
            mlua::Error::SyntaxError { message, .. } => message.clone(),
            mlua::Error::CallbackError { cause, .. } => msg(cause),
            mlua::Error::MemoryError(_) => "script used too much memory".into(),
            other => other.to_string(),
        }
    }
    let m = msg(&e);
    let m = m.split("\nstack traceback").next().unwrap_or(&m).trim().to_string();
    anyhow!(m)
}
