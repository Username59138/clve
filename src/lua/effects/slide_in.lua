-- Slides the layer in from outside the frame.
-- layer:effect("slide_in", { from = "bottom", len = 0.8 })
params = {
  from = "left",   -- left, right, top, bottom
  len = 0.6,       -- seconds
  easing = "ease_out",
  distance = 0,    -- pixels; 0 means the whole frame
}

function apply(t, layer, p)
  local f = easing[p.easing]
  if not f then error("slide_in: unknown easing '" .. tostring(p.easing) .. "'", 0) end
  local k = 1 - f(progress(t, 0, p.len))
  local dx, dy = 0, 0
  if p.from == "left" then dx = -1
  elseif p.from == "right" then dx = 1
  elseif p.from == "top" then dy = -1
  elseif p.from == "bottom" then dy = 1
  else error("slide_in: from must be left, right, top or bottom", 0) end
  local dist = p.distance
  if dist <= 0 then dist = (dx ~= 0) and project.width or project.height end
  layer.x = layer.x + dx * dist * k
  layer.y = layer.y + dy * dist * k
end
