-- Fades opacity (and volume) in at the start of the layer and out at the end.
-- layer:effect("fade", { enter = 0.5, exit = 1 })
params = {
  enter = 0.5, -- seconds
  exit = 0.5,  -- seconds
}

function apply(t, layer, p)
  local k = math.min(fade_in(t, 0, p.enter), fade_out(t, clip.duration - p.exit, p.exit))
  layer.opacity = layer.opacity * k
  layer.volume = layer.volume * k
end
