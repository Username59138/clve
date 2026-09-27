-- Pops in from nothing with a little overshoot.
-- layer:effect("pop", { len = 0.4 })
params = {
  len = 0.4, -- seconds
}

function apply(t, layer, p)
  layer.scale = layer.scale * math.max(0, easing.back_out(progress(t, 0, p.len)))
end
