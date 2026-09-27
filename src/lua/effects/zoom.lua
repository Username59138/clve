-- Slow zoom over the whole layer (Ken Burns).
-- layer:effect("zoom", { from = 1, to = 1.15 })
params = {
  from = 1.0,
  to = 1.1,
  easing = "linear",
}

function apply(t, layer, p)
  local f = easing[p.easing]
  if not f then error("zoom: unknown easing '" .. tostring(p.easing) .. "'", 0) end
  layer.scale = layer.scale * lerp(p.from, p.to, f(progress(t, 0, clip.duration)))
end
