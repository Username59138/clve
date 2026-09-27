-- Chromatic aberration: red and blue drift apart.
params = { amount = 6, angle = 0 }

function apply(t, layer, p)
  -- Lua can prepare params before the shader runs
  p.angle = p.angle + t * 90
end

margin = 10

shader = [[
fn effect(uv: vec2f) -> vec4f {
  let d = rotate(vec2f(p.amount * u.density, 0.0), p.angle) / u.resolution;
  let r = sample(uv + d);
  let g = sample(uv);
  let b = sample(uv - d);
  let a = max(max(r.a, g.a), b.a);
  return vec4f(r.r, g.g, b.b, a);
}
]]
