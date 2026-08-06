local patterns = {}

-- 24 unit directions, scaled by 256. Keeping the table literal avoids
-- near-threshold platform trig differences in collision-heavy patterns.
local dirs = {
  {256,0},{247,66},{222,128},{181,181},{128,222},{66,247},
  {0,256},{-66,247},{-128,222},{-181,181},{-222,128},{-247,66},
  {-256,0},{-247,-66},{-222,-128},{-181,-181},{-128,-222},{-66,-247},
  {0,-256},{66,-247},{128,-222},{181,-181},{222,-128},{247,-66},
}

local function wrap(index)
  return ((index - 1) % #dirs) + 1
end

local function velocity(index, speed)
  local direction = dirs[wrap(index)]
  return direction[1] * speed / 256, direction[2] * speed / 256
end

function patterns.ring(spawn, x, y, count, speed, phase, style)
  local step = #dirs / count
  for shot = 0, count - 1 do
    local vx, vy = velocity(phase + flr(shot * step), speed)
    spawn(x, y, vx, vy, style)
  end
end

function patterns.spiral(spawn, x, y, phase, speed, style)
  local vx, vy = velocity(phase, speed)
  spawn(x, y, vx, vy, style)
  local opposite_x, opposite_y = velocity(phase + 12, speed)
  spawn(x, y, opposite_x, opposite_y, style)
end

-- A rotating, visibly incomplete fan. Unlike a full ring it leaves a broad
-- safe side, so several releases can form a readable sweep instead of a wall.
function patterns.petal(spawn, x, y, phase, count, gap, speed, style)
  local center = (count - 1) / 2
  for shot = 0, count - 1 do
    local index = phase + flr((shot - center) * gap)
    local vx, vy = velocity(index, speed)
    spawn(x, y, vx, vy, style)
  end
end

function patterns.aimed(spawn, x, y, target_x, target_y, count, speed, spread, style)
  local dx, dy = target_x - x, target_y - y
  local length = math.sqrt(dx * dx + dy * dy)
  if length < 0.001 then dx, dy, length = 0, 1, 1 end
  local ux, uy = dx / length, dy / length
  local px, py = -uy, ux
  local center = (count - 1) / 2
  for shot = 0, count - 1 do
    local offset = (shot - center) * spread
    local sx, sy = ux + px * offset, uy + py * offset
    local shot_length = math.sqrt(sx * sx + sy * sy)
    spawn(x, y, sx / shot_length * speed, sy / shot_length * speed, style)
  end
end

function patterns.rain(spawn, frame, count, speed, style)
  for shot = 1, count do
    local lane = (frame * 37 + shot * 53) % 184
    local drift = ((frame + shot * 7) % 9 - 4) * 0.035
    spawn(4 + lane, 25 - (shot % 3) * 7, drift, speed, style)
  end
end

return patterns
