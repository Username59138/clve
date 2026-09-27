function frame(t, layer)
  layer.text = typewriter(layer.content, t, 0.04)
  layer:effect("shadow", { x = 0, y = 4, blur = 10, opacity = 0.8 })
  layer:effect("fade", { enter = 0, exit = 0.5 })
end
