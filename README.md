# clve

Command Line Video Editor. Edit video as a project: a folder of layers, TOML for settings and Lua for animation. Projects are plain text, so you can read them, keep them in git and generate them from scripts.

> Early stage: projects, layers, checking and the timeline work; rendering and Lua are in progress.

## Install

Requires Rust and ffmpeg (`ffprobe` is used to read media).

```
cargo install --path .
```

## Quick start

```
clve new video demo
cd demo
clve new layer video intro ~/clip.mp4
clve new layer text title "Hello"
clve check
clve timeline
```
