-- Neon-sign flicker: a few quick dips in brightness right after the start.
-- layer:effect("flicker", { until_t = 1.2 })
params = {
  until_t = 1.2, -- seconds of flickering
  seed = 3,
}

function apply(t, layer, p)
  if t < p.until_t and noise(t * 25, p.seed) > 0.35 then
    layer.opacity = layer.opacity * 0.25
  end
end
