function frame(t, layer)
  layer:effect("pop", { len = 0.5 })
  layer:effect("flicker")
  -- a little breathing after it lands
  if t > 0.5 then
    layer:effect("pulse", { amount = 0.02, speed = 0.5 })
  end
  -- neon: the glow swells in once the flicker settles
  layer:effect("glow", { radius = 28, amount = 0.9 * ease_out(t, 0.8, 1), threshold = 0.2 })
  layer:effect("fade", { enter = 0, exit = 0.6 })
end
