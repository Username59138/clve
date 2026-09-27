function frame(t, layer)
  layer.text = typewriter(layer.content, t, 0.05)
  -- blinking cursor while typing
  if utf8.len(layer.text) < utf8.len(layer.content) and math.floor(t * 4) % 2 == 0 then
    layer.text = layer.text .. "_"
  end
  layer:effect("fade", { enter = 0, exit = 0.6 })
end
