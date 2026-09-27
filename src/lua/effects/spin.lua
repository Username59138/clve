-- Constant rotation.
-- layer:effect("spin", { speed = 45 })
params = {
  speed = 90, -- degrees per second
}

function apply(t, layer, p)
  layer.rotation = layer.rotation + t * p.speed
end
