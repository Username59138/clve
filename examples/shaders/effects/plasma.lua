-- Generates a moving plasma, ignoring the layer's own pixels.
params = { speed = 0.5, scale = 3 }

shader = [[
fn effect(uv: vec2f) -> vec4f {
  let q = uv * p.scale;
  let t = u.time * p.speed;
  let v = sin(q.x * 3.0 + t) + sin(q.y * 4.0 - t * 1.3) + sin((q.x + q.y) * 2.0 + t * 0.7)
        + noise(q * 2.0 + vec2f(t, 0.0));
  let col = 0.5 + 0.5 * cos(vec3f(0.0, 2.1, 4.2) + v * 1.5);
  return vec4f(col * 0.6, 1.0);
}
]]
