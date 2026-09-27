function frame(t, layer)
  layer:effect("pop", { len = 0.6 })
  -- the wobble calms down over the first two seconds
  layer:effect("wave", { amount = 30 * fade_out(t, 0, 2) + 4, freq = 2 })
  layer:effect("rgb_split", { amount = 6 })
  layer:effect("glow", { radius = 30, amount = 0.7, threshold = 0.3 })
  layer:effect("fade", { enter = 0, exit = 0.5 })
end
