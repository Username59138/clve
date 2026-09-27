-- Camera-shake style jitter.
-- layer:effect("shake", { amount = 12, speed = 20 })
params = {
  amount = 10, -- pixels
  speed = 15,  -- wobbles per second
  seed = 1,
}

function apply(t, layer, p)
  layer.x = layer.x + wiggle(t, p.speed, p.amount, p.seed)
  layer.y = layer.y + wiggle(t, p.speed, p.amount, p.seed + 17)
end
