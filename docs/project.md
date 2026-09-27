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
type = "video"             # video, image, text, audio or color
source = "media/clip.mp4"  # video, image, audio
start = "0s"               # where the layer starts on the timeline
duration = "full"          # "full" (video/audio only) or a time
in = "2s"                  # skip the first 2 seconds of the source
z = 1                      # higher is drawn on top
```

Times can be written as `5s`, `1.5s`, `500ms`, `00:12` or `01:02:03`.

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

### By type

| type    | keys                                                                          |
|---------|-------------------------------------------------------------------------------|
| `video` | `source`, `volume` (1.0), `pan` (-1 left … 1 right)                          |
| `image` | `source` — png, jpeg, webp                                                    |
| `text`  | `content`, `font` (`sans-serif`, `serif`, `monospace` or an installed family), `size`, `color`, `align` (`left`, `center`, `right`) |
| `audio` | `source`, `volume`, `pan`                                                     |
| `color` | `color` — fills the frame; `#rrggbb` or `#rrggbbaa`                           |

## Commands

```
clve new video <name>                          new project
clve new layer <type> <name> <file|text|color> new layer (--link to not copy the file)
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
```
