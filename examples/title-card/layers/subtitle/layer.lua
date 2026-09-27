function frame(t, layer)
  layer:effect("slide_in", { from = "bottom", len = 0.8, distance = 120 })
  layer.opacity = fade_in(t, 0, 0.6)
  -- starts out of focus and sharpens as it arrives
  layer:effect("blur", { radius = 10 * fade_out(t, 0, 0.8) })
  layer:effect("shadow", { x = 0, y = 6, blur = 14, opacity = 0.7 })
  layer:effect("fade", { enter = 0, exit = 0.6 })
end
