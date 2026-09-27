-- Gentle breathing scale.
-- layer:effect("pulse", { amount = 0.1, speed = 2 })
params = {
  amount = 0.05, -- fraction of the size
  speed = 1,     -- pulses per second
}

function apply(t, layer, p)
  layer.scale = layer.scale * (1 + math.sin(t * p.speed * 2 * math.pi) * p.amount)
end
