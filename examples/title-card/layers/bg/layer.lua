-- the background slowly drifts from deep blue to violet
function frame(t, layer)
  layer.color = mix_color("#0f1226", "#2a1030", ease_in_out(t, 0, clip.duration))
end
