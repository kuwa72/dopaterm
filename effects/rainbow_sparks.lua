-- Sample dopaterm effect plugin.
-- Drop this file into <config dir>/dopaterm/effects/ (e.g. ~/.config/dopaterm/effects,
-- or %APPDATA%\dopaterm\effects on Windows).
--
-- API:
--   on_event(e)  -- called per terminal event.
--     e.type: "key" | "erased" | "cursor_moved" | "pty_output" | "bell"
--     key:          e.kind = "char"|"backspace"|"enter"|"arrow"|"other", e.x, e.y
--     erased:       e.ch, e.x, e.y, e.w, e.h, e.r, e.g, e.b
--     cursor_moved: e.x, e.y, e.w, e.h, e.dx, e.dy
--   fx.spawn{ x, y, vx, vy, ay, drag, life, size, r, g, b, a, kind, spin }
--     kind: 0 = solid rect, 1 = soft glow. spin!=0 rotates along velocity.

fx_name = "rainbow_sparks"

local hue = 0.0

local function hsv(h)
  local i = math.floor(h * 6) % 6
  local f = h * 6 - math.floor(h * 6)
  local q = { 1, f, 0, 0, 1 - f, 1 }
  local t = { 1 - f, 1, 1, f, 0, 0 }
  return q[i + 1], t[i + 1], ({ 0, 0, f, 1, 1, 1 - f })[i + 1]
end

function on_event(e)
  if e.type == "key" and e.kind == "char" then
    hue = (hue + 0.04) % 1.0
    local r, g, b = hsv(hue)
    for i = 1, 6 do
      local a = math.pi * 2 * i / 6
      fx.spawn{
        x = e.x, y = e.y,
        vx = math.cos(a) * 140, vy = math.sin(a) * 140 - 60,
        ay = 500, drag = 1.0,
        life = 0.5, size = 3.0,
        r = r, g = g, b = b, a = 1.0,
        kind = 1, spin = 0,
      }
    end
  elseif e.type == "bell" then
    fx.spawn{ x = e.x, y = e.y, vx = 0, vy = 0, ay = 0, life = 0.4,
              size = 28, r = 1, g = 0.3, b = 0.3, a = 0.35, kind = 1 }
  end
end
