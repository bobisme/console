-- Exact 48x48 Apollo64 moon for the live backdrop. The exported pixels are
-- converted to compact horizontal spans in ribbit-recoil.cart so the asset can
-- remain inspectable without consuming the four remaining atlas cells.
local function palette()
  return {
    {0, 0, 0, 0},
    {37, 58, 94, 255},    -- Apollo 1: outer halo
    {21, 29, 40, 255},    -- Apollo 50: halo break/shadow
    {129, 151, 150, 255}, -- Apollo 57: cool rim
    {208, 218, 145, 255}, -- Apollo 15: green-gold surface
    {231, 213, 179, 255}, -- Apollo 23: warm surface
    {108, 132, 134, 255}, -- Apollo 56: crater shadow
    {148, 166, 164, 255}, -- Apollo 58: crater midtone
    {235, 237, 233, 255}, -- Apollo 63: pin highlight
  }
end

local function blank(w, h)
  local rows = {}
  for y = 1, h do
    rows[y] = {}
    for x = 1, w do rows[y][x] = 0 end
  end
  return rows
end

local function pixel(rows, x, y, ink)
  if y >= 0 and y < #rows and x >= 0 and x < #rows[1] then
    rows[y + 1][x + 1] = ink
  end
end

local function rect(rows, x0, y0, x1, y1, ink)
  for y = y0, y1 do for x = x0, x1 do pixel(rows, x, y, ink) end end
end

local function circle(rows, cx, cy, r, ink)
  for y = cy - r, cy + r do
    for x = cx - r, cx + r do
      if (x - cx) * (x - cx) + (y - cy) * (y - cy) <= r * r then
        pixel(rows, x, y, ink)
      end
    end
  end
end

local function bands(rows, ink, specs)
  for i = 1, #specs do
    local s = specs[i]
    rect(rows, s[1], s[2], s[3], s[2], ink)
  end
end

local moon = blank(48, 48)
circle(moon, 24, 24, 23, 1)
circle(moon, 24, 24, 21, 2)
circle(moon, 24, 24, 20, 3)
circle(moon, 24, 24, 18, 5)

-- Irregular green-gold continental clusters.
bands(moon, 4, {
  {12,8,19},{10,9,21},{9,10,18},{10,11,16},
  {30,11,34},{29,12,36},{30,13,34},
  {35,19,38},{34,20,39},{35,21,37},
  {10,25,15},{9,26,17},{11,27,16},
  {24,24,28},{23,25,30},{25,26,29},
  {19,35,25},{18,36,27},{21,37,26},
})

-- Six non-circular crater regions, each made from staggered scanlines.
bands(moon, 6, {
  {15,13,18},{13,14,20},{12,15,19},{13,16,18},{15,17,17},
  {32,9,34},{30,10,36},{31,11,35},{32,12,34},
  {22,21,24},{20,22,25},{21,23,25},{22,24,23},
  {12,28,15},{10,29,17},{9,30,18},{11,31,17},{13,32,15},
  {31,32,35},{29,33,37},{28,34,38},{30,35,37},{32,36,35},
  {38,22,40},{36,23,40},{37,24,39},{38,25,39},
})
bands(moon, 7, {
  {15,13,17},{13,14,16},{32,9,33},{30,10,33},{21,22,23},
  {10,29,14},{29,33,34},{37,23,39},
})

-- Scattered surface mottling and tiny high points.
pixel(moon, 22, 8, 7)
pixel(moon, 27, 9, 8)
pixel(moon, 8, 20, 7)
pixel(moon, 16, 22, 4)
pixel(moon, 33, 18, 8)
pixel(moon, 24, 27, 7)
pixel(moon, 18, 31, 8)
pixel(moon, 25, 37, 7)
pixel(moon, 37, 27, 4)
pixel(moon, 14, 36, 7)
pixel(moon, 35, 35, 8)
pixel(moon, 7, 24, 4)

-- Break the two-value halo and roughen the circular edge by a pixel.
rect(moon, 22, 1, 26, 1, 0)
rect(moon, 5, 12, 6, 16, 0)
rect(moon, 41, 28, 46, 30, 0)
rect(moon, 14, 42, 18, 46, 0)
pixel(moon, 7, 9, 1)
pixel(moon, 42, 15, 1)
pixel(moon, 39, 39, 1)
pixel(moon, 10, 38, 3)
pixel(moon, 37, 8, 3)

local image = paintop.image.indexed {
  id = "authored_moon", width = 48, height = 48,
  palette = palette(), rows = moon,
}
local alpha = paintop.assert.binary_alpha {id = "moon_alpha", image = image.image}
local exact_palette = paintop.assert.palette {
  id = "moon_palette", image = image.image, palette = palette(),
  transparent_rgb = "zero",
}

return paintop.plan {
  name = "ribbit-recoil-authored-moon",
  description = "Irregular warm 48x48 moon with clustered craters, mottling and a broken two-value halo.",
  policy = {
    resources = {max_nodes = 4, max_pixels_per_resource = 4096, max_splats = 0},
    execution = {deadline_ms = 30000, allowed_backends = {"cpu-reference"}},
  },
  nodes = {image, alpha, exact_palette},
  assertions = {
    {id = "moon_alpha", subject = alpha.report},
    {id = "moon_palette", subject = exact_palette.report},
  },
  exports = {
    moon = {resource = image.image, kind = "image", path = "moon.png", encoding = {format = "png"}},
  },
  evidence = {
    trace = "detailed", graph = {"dot"}, contact_sheet = true,
    materialize = {image.image}, diffs = paintop.array {},
  },
}
