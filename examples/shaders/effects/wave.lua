-- Wobbly water distortion.
params = {
  amount = 12,   -- pixels
  freq = 3,      -- waves across the layer
  speed = 2,
}

margin = 16

shader = [[
fn effect(uv: vec2f) -> vec4f {
  let px = p.amount * u.density / u.resolution.x;
  let shift = sin(uv.y * p.freq * 2.0 * PI + u.time * p.speed * 2.0 * PI) * px;
  return sample(uv + vec2f(shift, 0.0));
}
]]
