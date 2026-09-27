function frame(t, layer)
  layer:effect("slide_in", { from = "bottom", len = 0.8, distance = 120 })
  layer.opacity = fade_in(t, 0, 0.6)
  layer:effect("fade", { enter = 0, exit = 0.6 })
end
