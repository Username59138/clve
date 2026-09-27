# Lua API

Each layer can have a `layer.lua` with a `frame` function:

```lua
function frame(t, layer)
  layer.opacity = fade_in(t, 0, 0.5)
  layer.y = 900 - ease_out(t, 0, 0.6) * 100
  layer:effect("shake", { amount = 4 })
end
```

`frame` is called for every frame. `t` is the time in seconds since the layer
started. Before each call `layer` is reset to the values from `layer.toml`, so
the result depends only on `t`: seeking, `--from`, single-frame renders and the
final video always agree.

Scripts run in a sandbox: `math`, `string`, `table` and `utf8` are available;
there is no file, network or OS access. A frame that takes longer than 2
seconds (for example an endless loop) stops the render with an error.

`print(...)` writes to the terminal, prefixed with the layer name.

## layer

| field      | types           | notes                                             |
|------------|-----------------|---------------------------------------------------|
| `x`, `y`   | all visual      | anchor position in pixels                         |
| `scale`    | all visual      | negative values mirror                            |
| `rotation` | all visual      | degrees, clockwise                                |
| `opacity`  | all visual      | 0..1                                              |
| `anchor`   | all visual      | a name like `"bottom"` or `{x, y}` as fractions   |
| `visible`  | all visual      | `false` hides the layer for this frame            |
| `volume`   | video, audio    | animated volume is applied smoothly               |
| `pan`      | video, audio    | -1 left … 1 right                                 |
| `text`     | text            | what is drawn                                     |
| `content`  | text            | the original text from layer.toml (read it, don't change it) |
| `font`, `size`, `align` | text   |                                               |
| `color`    | text, color     | `"#rrggbb"` or `"#rrggbbaa"`                      |

## Globals

```lua
project.width, project.height, project.fps, project.duration
clip.name, clip.type, clip.start, clip.duration
clip.global_t   -- time on the whole timeline
```

## Helpers

```lua
lerp(a, b, k)
clamp(x, lo, hi)
progress(t, start, len)        -- 0..1 through [start, start + len]

fade_in(t, start, len)         -- 0 → 1, linear
fade_out(t, start, len)        -- 1 → 0
ease_in(t, start, len)         -- 0 → 1, eased; also ease_out, ease_in_out
easing.back_out(k)             -- raw curves: linear, ease_in, ease_out, ease_in_out,
                               -- back_out, elastic_out, bounce_out

keyframes(t, {
  { 0,   0 },
  { 1.5, 100, "ease_out" },    -- easing of the segment that ends here
  { 3,   { 50, 20 } },         -- values can be numbers or lists of numbers
})

typewriter(text, t, per_char, start)
noise(x, seed)                 -- smooth, -1..1
wiggle(t, freq, amount, seed)

rgb(255, 128, 0)               -- "#ff8000"; rgb(r, g, b, a) for alpha
mix_color("#ffffff", "#ff0000", 0.5)
```

## Effects

```lua
layer:effect("fade", { enter = 1, exit = 0.5 })
```

Effects run immediately, in the order they are called, and change `layer` just
like the rest of the script.

### Motion

Written in Lua (the source is in `src/lua/effects/`), they change position,
size, rotation and opacity.

| name       | params                                             |
|------------|----------------------------------------------------|
| `fade`     | `enter = 0.5`, `exit = 0.5` — also fades volume    |
| `pop`      | `len = 0.4`                                        |
| `pulse`    | `amount = 0.05`, `speed = 1`                       |
| `shake`    | `amount = 10`, `speed = 15`, `seed = 1`            |
| `slide_in` | `from = "left"`, `len = 0.6`, `easing = "ease_out"`, `distance = 0` |
| `spin`     | `speed = 90`                                       |
| `zoom`     | `from = 1.0`, `to = 1.1`, `easing = "linear"`      |

### Pixels

Built into clve and run in Rust. They change the layer's image before it is
placed in the frame, so `blur` on a title blurs only the title. They apply in
the order you call them, and their parameters can be animated like anything
else: `layer:effect("blur", { radius = 20 * fade_out(t, 0, 1) })`.

Lengths are in project pixels, so a preview looks the same as the full render,
just smaller.

| name          | params                                                        | notes |
|---------------|---------------------------------------------------------------|-------|
| `brightness`  | `amount = 1.2`                                                | 1 = unchanged |
| `contrast`    | `amount = 1.2`                                                | 1 = unchanged |
| `saturation`  | `amount = 1.5`                                                | 0 = gray, 1 = unchanged |
| `hue`         | `degrees = 30`                                                | rotates colors |
| `grayscale`   | `amount = 1`                                                  | |
| `sepia`       | `amount = 1`                                                  | |
| `invert`      | `amount = 1`                                                  | |
| `tint`        | `color = "#ff8800"`, `amount = 0.3`                           | |
| `temperature` | `amount = 0.3`                                                | warmer above 0, cooler below |
| `blur`        | `radius = 8`                                                  | |
| `sharpen`     | `amount = 0.6`, `radius = 2`                                  | |
| `pixelate`    | `size = 16`                                                   | |
| `vignette`    | `amount = 0.5`, `radius = 0.5`, `softness = 0.5`              | darkens the edges |
| `grain`       | `amount = 0.06`, `size = 1.5`, `speed = 12`, `seed = 0`       | `speed` = new grain per second, 0 = still |
| `glow`        | `radius = 20`, `amount = 0.8`, `threshold = 0.6`              | bright parts bleed light |
| `shadow`      | `x = 8`, `y = 8`, `blur = 12`, `color = "#000000"`, `opacity = 0.6` | |
| `chroma_key`  | `color = "#00ff00"`, `similarity = 0.25`, `smoothness = 0.08`, `spill = 0.5` | green screen |
| `rounded`     | `radius = 24`                                                 | rounded corners |
| `crop`        | `left`, `top`, `right`, `bottom` = 0                          | in pixels, the layer stays in place |

Moving grain is noise that changes every frame, and noise hardly compresses:
it makes encoding slower and files bigger, in clve as in any editor. Use
`speed = 0` for still grain, or keep it for the final render.

`clve list effects` shows every effect with its parameters.

### Shaders

An effect file can also carry a WGSL shader that runs on the GPU — see
[shaders.md](shaders.md).

### Writing an effect

`clve new effect glitch` creates `effects/glitch.lua`:

```lua
params = {
  every = 0.7,
  power = 40,
}

function apply(t, layer, p)
  if t % p.every < 0.08 then
    layer.x = layer.x + noise(math.floor(t / p.every), 7) * p.power
  end
end
```

`params` holds the defaults; passing a parameter that is not listed there is an
error, so typos don't go unnoticed. A project effect with the same name as a
built-in one replaces it.

Your effects can use the built-in ones: `layer:effect("blur", { radius = 4 })`
inside `apply` works the same as in `layer.lua`.
