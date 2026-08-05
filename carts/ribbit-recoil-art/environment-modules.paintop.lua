-- Exact Apollo64 environment study composed from reusable pixel modules.
-- This is intentionally a construction target rather than generated game art:
-- every pixel is authored by the bounded Lua frontend, normalized to canonical
-- JSON, stamped without filtering, and guarded by palette/alpha assertions.
local function palette()
  return {
    {0, 0, 0, 0},
    {9, 10, 20, 255},
    {16, 20, 31, 255},
    {21, 29, 40, 255},
    {37, 58, 94, 255},
    {60, 94, 139, 255},
    {44, 60, 67, 255},
    {72, 94, 99, 255},
    {148, 166, 164, 255},
    {199, 207, 204, 255},
    {232, 193, 112, 255},
    {91, 47, 104, 255},
    {122, 54, 123, 255},
    {95, 167, 199, 255},
    {164, 221, 219, 255},
    {207, 87, 60, 255},
  }
end

local function blank(width, height, ink)
  local rows = {}
  for y = 1, height do
    local row = {}
    for x = 1, width do row[x] = ink end
    rows[y] = row
  end
  return rows
end

local function pixel(rows, x, y, ink)
  if y >= 0 and y < #rows and x >= 0 and x < #rows[1] then
    rows[y + 1][x + 1] = ink
  end
end

local function rect(rows, x0, y0, x1, y1, ink)
  for y = y0, y1 do
    for x = x0, x1 do pixel(rows, x, y, ink) end
  end
end

local function line(rows, x0, y0, x1, y1, ink)
  local dx = math.abs(x1 - x0)
  local sx = x0 < x1 and 1 or -1
  local dy = -math.abs(y1 - y0)
  local sy = y0 < y1 and 1 or -1
  local err = dx + dy
  while true do
    pixel(rows, x0, y0, ink)
    if x0 == x1 and y0 == y1 then break end
    local twice = err * 2
    if twice >= dy then err = err + dy; x0 = x0 + sx end
    if twice <= dx then err = err + dx; y0 = y0 + sy end
  end
end

local function circle(rows, cx, cy, radius, ink, filled)
  for y = cy - radius, cy + radius do
    for x = cx - radius, cx + radius do
      local d = (x - cx) * (x - cx) + (y - cy) * (y - cy)
      if (filled and d <= radius * radius)
          or (not filled and d <= radius * radius and d >= (radius - 1) * (radius - 1)) then
        pixel(rows, x, y, ink)
      end
    end
  end
end

local function indexed(id, rows)
  return paintop.image.indexed {
    id = id,
    width = #rows[1],
    height = #rows,
    palette = palette(),
    rows = rows,
  }
end

local sky_rows = blank(192, 304, 1)
for i = 0, 31 do
  local x = (i * 47 + 13) % 190
  local y = 8 + (i * 29) % 126
  pixel(sky_rows, x, y, i % 7 == 0 and 9 or i % 3 == 0 and 5 or 4)
end
local sky = indexed("sky", sky_rows)

local cloud_rows = blank(48, 16, 0)
rect(cloud_rows, 2, 9, 45, 12, 2)
circle(cloud_rows, 8, 8, 5, 2, true)
circle(cloud_rows, 19, 6, 7, 2, true)
circle(cloud_rows, 31, 7, 6, 2, true)
circle(cloud_rows, 41, 9, 4, 2, true)
line(cloud_rows, 3, 12, 44, 12, 3)
line(cloud_rows, 13, 3, 23, 1, 3)
local cloud = indexed("cloud", cloud_rows)

local moon_rows = blank(56, 56, 0)
circle(moon_rows, 28, 28, 27, 3, true)
circle(moon_rows, 28, 28, 24, 9, true)
circle(moon_rows, 18, 19, 7, 8, true)
circle(moon_rows, 36, 34, 6, 7, true)
circle(moon_rows, 36, 15, 4, 8, true)
circle(moon_rows, 15, 36, 4, 8, false)
circle(moon_rows, 28, 28, 24, 9, false)
pixel(moon_rows, 41, 24, 14)
pixel(moon_rows, 25, 43, 14)
local moon = indexed("moon", moon_rows)

local far_rows = blank(24, 92, 0)
rect(far_rows, 1, 17, 22, 91, 3)
rect(far_rows, 4, 21, 19, 91, 2)
rect(far_rows, 7, 8, 16, 17, 3)
line(far_rows, 8, 7, 15, 7, 5)
line(far_rows, 12, 7, 12, 1, 4)
for y = 29, 84, 11 do
  for x = 7, 17, 7 do
    if (x + y) % 3 ~= 0 then
      rect(far_rows, x, y, x + 1, y + 2, (x + y) % 2 == 0 and 10 or 5)
    end
  end
end
local far = indexed("far_building", far_rows)

local facade_rows = blank(44, 132, 0)
rect(facade_rows, 0, 9, 43, 131, 2)
rect(facade_rows, 3, 13, 40, 131, 3)
rect(facade_rows, 7, 18, 36, 125, 1)
rect(facade_rows, 0, 7, 43, 11, 6)
line(facade_rows, 2, 7, 41, 7, 9)
for x = 7, 37, 10 do
  rect(facade_rows, x, 18, x + 2, 124, 6)
  line(facade_rows, x + 2, 19, x + 2, 123, 7)
end
for y = 25, 112, 15 do
  line(facade_rows, 5, y, 38, y, 6)
  for x = 10, 32, 11 do
    rect(facade_rows, x, y + 4, x + 3, y + 9, (x + y) % 3 == 0 and 10 or 11)
    pixel(facade_rows, x, y + 4, 9)
  end
end
rect(facade_rows, 7, 112, 36, 126, 2)
rect(facade_rows, 13, 114, 29, 126, 11)
line(facade_rows, 14, 115, 28, 115, 12)
rect(facade_rows, 5, 128, 38, 131, 1)
for y = 29, 106, 23 do
  pixel(facade_rows, 4, y, 15)
  pixel(facade_rows, 39, y + 3, 10)
end
local facade = indexed("facade", facade_rows)

local tank_rows = blank(36, 74, 0)
rect(tank_rows, 3, 20, 32, 66, 6)
circle(tank_rows, 18, 20, 15, 6, true)
rect(tank_rows, 4, 24, 16, 65, 3)
circle(tank_rows, 18, 20, 14, 8, false)
line(tank_rows, 5, 29, 31, 29, 8)
line(tank_rows, 4, 53, 32, 53, 7)
rect(tank_rows, 10, 36, 25, 47, 2)
rect(tank_rows, 12, 38, 23, 45, 11)
line(tank_rows, 13, 41, 22, 41, 12)
line(tank_rows, 7, 66, 3, 73, 7)
line(tank_rows, 28, 66, 32, 73, 6)
for y = 31, 62, 8 do pixel(tank_rows, 30, y, 10) end
local tank = indexed("tank", tank_rows)

local catwalk_rows = blank(56, 30, 0)
rect(catwalk_rows, 0, 0, 55, 5, 7)
line(catwalk_rows, 1, 0, 54, 0, 9)
rect(catwalk_rows, 3, 6, 52, 10, 2)
for x = 4, 48, 12 do
  line(catwalk_rows, x, 10, x, 29, 6)
  line(catwalk_rows, x + 9, 10, x, 29, 7)
  line(catwalk_rows, x, 10, x + 9, 29, 3)
  pixel(catwalk_rows, x + 2, 3, x % 3 == 0 and 15 or 10)
end
local catwalk = indexed("catwalk", catwalk_rows)

local sewer_rows = blank(192, 40, 1)
rect(sewer_rows, 0, 0, 191, 7, 3)
line(sewer_rows, 0, 0, 191, 0, 9)
for x = 0, 191, 16 do
  line(sewer_rows, x, 3, x + 8, 3, 7)
  pixel(sewer_rows, x + 11, 5, 15)
end
rect(sewer_rows, 0, 11, 191, 39, 2)
for x = 8, 183, 31 do
  line(sewer_rows, x, 20, x + 10, 20, x % 2 == 0 and 13 or 5)
  line(sewer_rows, x + 4, 28, x + 15, 28, 4)
end
circle(sewer_rows, 96, 14, 13, 6, true)
circle(sewer_rows, 96, 14, 11, 1, true)
rect(sewer_rows, 90, 15, 102, 39, 1)
for x = 91, 101, 5 do line(sewer_rows, x, 17, x, 35, 7) end
local sewer = indexed("sewer", sewer_rows)

local first = paintop.image.stamp {
  id = "atmosphere",
  base = sky.image,
  source = far.image,
  placements = {
    {x = 0, y = 127}, {x = 23, y = 145, flip_x = true},
    {x = 46, y = 117}, {x = 69, y = 138, flip_x = true},
    {x = 92, y = 124}, {x = 115, y = 151, flip_x = true},
    {x = 138, y = 113}, {x = 161, y = 142, flip_x = true},
  },
  mode = "binary-alpha-over",
  bounds = "clip",
}

local clouds = paintop.image.stamp {
  id = "clouds",
  base = first.image,
  source = cloud.image,
  placements = {
    {x = 4, y = 51}, {x = 63, y = 76, flip_x = true},
    {x = 121, y = 60}, {x = 27, y = 101, flip_x = true},
  },
  mode = "binary-alpha-over",
  bounds = "clip",
}

local moonlit = paintop.image.stamp {
  id = "moonlit",
  base = clouds.image,
  source = moon.image,
  placements = {{x = 126, y = 14}},
  mode = "binary-alpha-over",
  bounds = "error",
}

local buildings = paintop.image.stamp {
  id = "buildings",
  base = moonlit.image,
  source = facade.image,
  placements = {
    {x = -2, y = 112}, {x = 38, y = 148, flip_x = true},
    {x = 148, y = 126, flip_x = true},
  },
  mode = "binary-alpha-over",
  bounds = "clip",
}

local tanks = paintop.image.stamp {
  id = "tanks",
  base = buildings.image,
  source = tank.image,
  placements = {{x = 80, y = 161}, {x = 111, y = 189, flip_x = true}},
  mode = "binary-alpha-over",
  bounds = "error",
}

local routes = paintop.image.stamp {
  id = "routes",
  base = tanks.image,
  source = catwalk.image,
  placements = {
    {x = 2, y = 101}, {x = 69, y = 137},
    {x = 133, y = 92}, {x = 105, y = 236, flip_x = true},
  },
  mode = "binary-alpha-over",
  bounds = "clip",
}

local final = paintop.image.stamp {
  id = "final",
  base = routes.image,
  source = sewer.image,
  placements = {{x = 0, y = 264}},
  mode = "copy",
  bounds = "error",
}

local binary_alpha = paintop.assert.binary_alpha {
  id = "binary_alpha",
  image = final.image,
}
local palette_check = paintop.assert.palette {
  id = "palette",
  image = final.image,
  palette = palette(),
  transparent_rgb = "zero",
}

return paintop.plan {
  name = "ribbit-recoil-environment-modules",
  description = "Exact modular Apollo64 construction study for dense moonlit industrial environments.",
  policy = {
    resources = {max_nodes = 24, max_pixels_per_resource = 100000, max_splats = 0},
    execution = {deadline_ms = 30000, allowed_backends = {"cpu-reference"}},
  },
  nodes = {
    sky, cloud, moon, far, facade, tank, catwalk, sewer,
    first, clouds, moonlit, buildings, tanks, routes, final,
    binary_alpha, palette_check,
  },
  assertions = {
    {id = "binary_alpha", subject = binary_alpha.report},
    {id = "palette", subject = palette_check.report},
  },
  exports = {
    final = {resource = final.image, kind = "image", path = "environment-modules-study.png", encoding = {format = "png"}},
  },
  evidence = {
    trace = "detailed",
    graph = {"dot"},
    contact_sheet = true,
    materialize = {cloud.image, moon.image, far.image, facade.image, tank.image, catwalk.image, sewer.image, final.image},
    diffs = paintop.array {},
  },
}
