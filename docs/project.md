# Project format

A clve project is a folder:

```
demo/
├── project.toml      render settings
├── layers/
│   └── intro/
│       ├── layer.toml    what the layer is and where it sits
│       └── layer.lua     how it moves (optional)
├── media/            imported files
├── effects/          your own effects, one .lua file each
└── out/              renders (ignored by git)
```

Commands work from any folder inside the project, like `cargo`.

## project.toml

```toml
[project]
name = "demo"

[render]
resolution = [1920, 1080]
fps = 30
codec = "h264"     # h264 or h265
quality = "high"   # low, medium, high or lossless
output = "out/"
# crf = 18         # overrides quality
```

## layer.toml

```toml
[layer]
type = "video"             # video, image, text, audio, color or adjust
source = "media/clip.mp4"  # video, image, audio
start = "0s"               # where the layer starts on the timeline
duration = "full"          # "full" (video/audio only) or a time
in = "2s"                  # skip the first 2 seconds of the source
out = "4.5s"               # stop the source here (optional)
z = 1                      # higher is drawn on top
```

Times can be written as `5s`, `1.5s`, `500ms`, `00:12` or `01:02:03`.

### Playing a piece of a source

`in` and `out` cut a piece out of a video or audio file. When the piece is
over and the layer is still on screen, its last frame stays and the sound is
silent — handy for someone who has finished talking but should not vanish.

`hold_start` does the same at the other end: the first frame is shown, silent,
for that long before the source starts playing. The hold is part of `duration`.

```toml
in = "1.08s"
out = "2.7s"
hold_start = "0.68s"   # stands still for 0.68 s, then says his line
start = "0s"
duration = "4.3s"      # 0.68 s hold + 1.62 s line + the last frame till 4.3 s
```

With `duration = "full"` the length is `hold_start + (out or end of file) - in`.

Two layers with the same `z` must not play at the same time — `clve check`
reports it. Audio layers have their own `z` tracks, so music on `z = 0` does
not clash with video on `z = 0`.

### Position and look

All optional. By default a layer sits in the middle of the frame.

| key        | default                      | meaning                                         |
|------------|------------------------------|-------------------------------------------------|
| `x`, `y`   | center of the frame          | where the anchor point goes, in pixels          |
| `anchor`   | `"center"`                   | `top-left`, `top`, `top-right`, `left`, `right`, `bottom-left`, `bottom`, `bottom-right` |
| `scale`    | `1.0`                        |                                                 |
| `rotation` | `0`                          | degrees, clockwise                              |
| `opacity`  | `1.0`                        | 0..1                                            |
| `fit`      | `contain` video, `none` image | `contain`, `cover`, `stretch`, `none`          |
| `flip_x`, `flip_y` | `false`              | mirror left↔right / top↔bottom, around the layer's own box |
| `blend`    | `"normal"`                   | how the layer mixes with what is below: `normal`, `screen`, `add`, `multiply`, `lighten`, `darken`, `overlay` |

`blend = "screen"` (or `"add"`) makes black transparent and lets light through:
use it for glows, fire, sparks or anything filmed on a black background.
`multiply` does the opposite — white disappears, dark stays.

### By type

| type    | keys                                                                          |
|---------|-------------------------------------------------------------------------------|
| `video` | `source`, `volume` (1.0), `pan` (-1 left … 1 right), `in`, `out`, `hold_start`, `lowpass`, `highpass` |
| `image` | `source` — png, jpeg, webp                                                    |
| `text`  | `content`, `font` (`sans-serif`, `serif`, `monospace` or an installed family), `size`, `color`, `align` (`left`, `center`, `right`) |
| `audio` | `source`, `volume`, `pan`, `in`, `out`, `hold_start`, `lowpass`, `highpass`   |
| `color` | `color` — fills the frame; `#rrggbb` or `#rrggbbaa`                           |
| `adjust`| nothing to draw: its pixel effects run over every layer below it              |

`lowpass` and `highpass` are audio filters, the cutoff in Hz (`0` = off).
`lowpass = 800` sounds muffled, like through a wall; animate it from Lua to
make the sound fade out of focus.

### Adjust layers

An adjust layer has no picture of its own. The effects its `layer.lua` calls
are applied to the whole frame as it is drawn so far — every layer with a
lower `z` — while layers above it stay untouched. Put a caption above it and
it stays sharp while everything else is pixelated:

```toml
[layer]
type = "adjust"
start = "10s"
duration = "5s"
z = 10          # the caption sits at z = 11
```

```lua
function frame(t, layer)
  layer:effect("pixelate", { size = 4 + math.floor(t) * 3 })
end
```

`opacity` below 1 mixes the result with the untouched frame.

## Commands

```
clve new video <name>                          new project
clve new layer <type> <name> <file|text|color|duration> new layer (--link to not copy the file)
clve new effect <name>                         new effects/<name>.lua

clve list layers [--json]
clve list effects
clve rm layer <name>
clve rm effect <name>

clve check [--json]                            validate everything, run scripts
clve timeline [--width N] [--json]             ASCII timeline

clve render                                    → out/<name>.mp4
clve render -o final.mp4
clve render --from 10s --to 30s                only a part
clve render --preview                          480p, fast
clve render --frame 3.5 -o frame.png           one frame as PNG
clve render -o clip.webm                       VP9 + Opus
clve render -o clip.gif --width 640            animated GIF (no sound)
```

The format follows the extension of `-o`: `.mp4`, `.mov`, `.mkv`, `.webm` or `.gif`.
`--width` scales the output, keeping the project's aspect ratio.
