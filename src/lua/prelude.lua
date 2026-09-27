-- clve standard library. Runs before every layer script.
-- Everything here is a plain global so scripts can use it directly.

local load = load -- kept private: scripts themselves cannot load code

function clamp(x, lo, hi)
  if x < lo then return lo end
  if x > hi then return hi end
  return x
end

function lerp(a, b, k)
  return a + (b - a) * k
end

-- 0..1 progress of t through [start, start + len]
function progress(t, start, len)
  start = start or 0
  if len == nil or len <= 0 then
    return t >= start and 1 or 0
  end
  return clamp((t - start) / len, 0, 1)
end

easing = {
  linear = function(k) return k end,
  ease_in = function(k) return k * k * k end,
  ease_out = function(k) local u = 1 - k; return 1 - u * u * u end,
  ease_in_out = function(k)
    if k < 0.5 then return 4 * k * k * k end
    local u = -2 * k + 2
    return 1 - u * u * u / 2
  end,
  back_out = function(k)
    local c1 = 1.70158
    local c3 = c1 + 1
    return 1 + c3 * (k - 1) ^ 3 + c1 * (k - 1) ^ 2
  end,
  elastic_out = function(k)
    if k == 0 or k == 1 then return k end
    return 2 ^ (-10 * k) * math.sin((k * 10 - 0.75) * (2 * math.pi) / 3) + 1
  end,
  bounce_out = function(k)
    local n, d = 7.5625, 2.75
    if k < 1 / d then return n * k * k
    elseif k < 2 / d then k = k - 1.5 / d; return n * k * k + 0.75
    elseif k < 2.5 / d then k = k - 2.25 / d; return n * k * k + 0.9375
    else k = k - 2.625 / d; return n * k * k + 0.984375 end
  end,
}

local function easing_fn(name, level)
  local f = easing[name or "linear"]
  if not f then error("unknown easing '" .. tostring(name) .. "'", level + 1) end
  return f
end

function ease_in(t, start, len) return easing.ease_in(progress(t, start, len)) end
function ease_out(t, start, len) return easing.ease_out(progress(t, start, len)) end
function ease_in_out(t, start, len) return easing.ease_in_out(progress(t, start, len)) end
function fade_in(t, start, len) return progress(t, start, len) end
function fade_out(t, start, len) return 1 - progress(t, start, len) end

-- keyframes(t, { {0, 0}, {1.5, 100, "ease_out"}, {3, 50} })
-- The easing on a key applies to the segment that ends at that key.
-- Values can be numbers or lists of numbers, e.g. {x, y}.
function keyframes(t, keys)
  if type(keys) ~= "table" or #keys == 0 then
    error("keyframes: expected a non-empty list of {time, value}", 2)
  end
  if t <= keys[1][1] then return keys[1][2] end
  for i = 2, #keys do
    local a, b = keys[i - 1], keys[i]
    if t < b[1] then
      local k = easing_fn(b[3], 2)((t - a[1]) / (b[1] - a[1]))
      if type(a[2]) == "table" then
        local out = {}
        for j = 1, #a[2] do out[j] = lerp(a[2][j], b[2][j], k) end
        return out
      end
      return lerp(a[2], b[2], k)
    end
  end
  return keys[#keys][2]
end

-- Text appearing letter by letter, `per_char` seconds per letter.
function typewriter(text, t, per_char, start)
  local n = math.floor((t - (start or 0)) / per_char)
  if n <= 0 then return "" end
  local len = utf8.len(text) or #text
  if n >= len then return text end
  return text:sub(1, utf8.offset(text, n + 1) - 1)
end

-- Smooth deterministic noise in -1..1.
local function hash(i, seed)
  local x = math.sin(i * 127.1 + seed * 311.7) * 43758.5453
  return (x - math.floor(x)) * 2 - 1
end

function noise(x, seed)
  seed = seed or 0
  local i = math.floor(x)
  local f = x - i
  local u = f * f * (3 - 2 * f)
  return lerp(hash(i, seed), hash(i + 1, seed), u)
end

function wiggle(t, freq, amount, seed)
  return noise(t * freq, seed or 0) * amount
end

-- Colors are "#rrggbb" / "#rrggbbaa" strings.
function rgb(r, g, b, a)
  local function c(v) return math.floor(clamp(v, 0, 255) + 0.5) end
  if a then return string.format("#%02x%02x%02x%02x", c(r), c(g), c(b), c(a)) end
  return string.format("#%02x%02x%02x", c(r), c(g), c(b))
end

local function parse_hex(s)
  local r, g, b, a = s:match("^#(%x%x)(%x%x)(%x%x)(%x?%x?)$")
  if not r then error("not a color: " .. tostring(s), 3) end
  return tonumber(r, 16), tonumber(g, 16), tonumber(b, 16), (a ~= "" and tonumber(a, 16) or 255)
end

function mix_color(c1, c2, k)
  local r1, g1, b1, a1 = parse_hex(c1)
  local r2, g2, b2, a2 = parse_hex(c2)
  return rgb(lerp(r1, r2, k), lerp(g1, g2, k), lerp(b1, b2, k), lerp(a1, a2, k))
end

-- ---------- effects ----------

local effect_cache = {}

local function get_effect(name)
  local fx = effect_cache[name]
  if fx then return fx end
  local src, chunk = __clve_effect_source(name)
  local env = setmetatable({}, { __index = _G })
  local fn, err = load(src, chunk, "t", env)
  if not fn then error(err, 0) end
  fn()
  if type(env.apply) ~= "function" then
    error("effect '" .. name .. "' does not define apply(t, layer, params)", 0)
  end
  fx = { apply = env.apply, params = env.params or {} }
  effect_cache[name] = fx
  return fx
end

__clve_layer_methods = {
  -- layer:effect("shake", { amount = 8 })
  effect = function(self, name, params)
    local fx = get_effect(name)
    local p = {}
    for k, v in pairs(fx.params) do p[k] = v end
    for k, v in pairs(params or {}) do
      if fx.params[k] == nil then
        error(string.format("effect '%s' has no parameter '%s'", name, tostring(k)), 2)
      end
      p[k] = v
    end
    fx.apply(__clve_t, self, p)
  end,
}
