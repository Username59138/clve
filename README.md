# clve

Command Line Video Editor. Edit video as a project: a folder of layers, TOML for settings and Lua for animation. Projects are plain text, so you can read them, keep them in git and generate them from scripts.

![title card rendered by clve](docs/demo.gif)

<sub>Rendered from [`examples/title-card`](examples/title-card) with `clve render -o demo.gif --width 640`.</sub>

## Install

Requires Rust and ffmpeg. Shader effects also need a GPU driver (Vulkan, Metal, DX12 or OpenGL).

```
cargo install --path .
```

## Quick start

```
clve new video demo
cd demo
clve new layer video intro ~/clip.mp4
clve new layer text title "Hello"
clve render
```

Then open `layers/title/layer.lua` and make it move:

```lua
function frame(t, layer)
  layer.text = typewriter(layer.content, t, 0.05)
  layer:effect("fade", { enter = 0.3, exit = 0.5 })
end
```

`clve render --frame 2s -o frame.png` shows a single frame, `clve render --preview` makes a quick 480p draft, `-o clip.webm` or `-o clip.gif` picks another format.

## Docs

- [Project format and commands](docs/project.md)
- [Lua API](docs/lua.md)
- [Shader effects](docs/shaders.md)
