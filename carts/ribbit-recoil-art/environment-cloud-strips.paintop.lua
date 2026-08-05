-- Three exact Apollo64 cloud strips for the live parallax backdrop. Their final
-- pixels are converted to compact horizontal spans in ribbit-recoil.cart; the
-- PNGs and Paintop evidence remain the inspectable authoring source.
local function palette()
  return {
    {0, 0, 0, 0},
    {37, 58, 94, 255},   -- Apollo 1: readable cloud body
    {60, 94, 139, 255},  -- Apollo 2: upper lobe response
    {117, 162, 180, 255},-- Apollo 3: sparse moonward rim
    {21, 29, 40, 255},   -- Apollo 50: broken underside shadow
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

local function indexed(id, rows)
  return paintop.image.indexed {
    id = id, width = #rows[1], height = #rows,
    palette = palette(), rows = rows,
  }
end

local function bands(rows, ink, specs)
  for i = 1, #specs do
    local s = specs[i]
    rect(rows, s[1], s[2], s[3], s[2], ink)
  end
end

local a = blank(96, 24)
-- One broad 90-pixel storm bank. Every scanline is placed deliberately so
-- the crown has unequal peaks without revealing circle-stamp cadence.
bands(a, 1, {
  {47,2,60},{32,3,39},{43,3,68},{18,4,27},{30,4,75},{80,4,85},
  {10,5,27},{29,5,81},{78,5,90},{6,6,91},{2,7,94},{0,8,95},
  {0,9,95},{1,10,94},{3,11,92},{6,12,90},{10,13,88},{14,14,85},
  {19,15,39},{44,15,66},{72,15,83},{24,16,34},{49,16,59},{75,16,80},
})
bands(a, 2, {
  {46,4,62},{32,5,43},{45,5,70},{16,6,33},{38,6,76},{10,7,29},
  {36,7,55},{62,7,82},{7,8,24},{31,8,50},{67,8,87},{14,9,38},
  {52,9,75},{8,11,28},{39,11,61},{70,11,86},
})
bands(a, 3, {{49,3,58},{34,4,41},{19,5,25},{64,5,72},{12,6,17},{81,6,87}})
bands(a, 4, {{6,13,18},{29,13,47},{61,13,76},{14,14,25},{39,14,53},{67,14,85},{19,15,39},{44,15,66},{72,15,83},{24,16,34},{49,16,59},{75,16,80}})

local b = blank(96, 24)
-- A lower 68-pixel bank with a wind-sheared left edge and torn bottom.
bands(b, 1, {
  {56,4,64},{30,5,38},{51,5,70},{23,6,42},{48,6,74},{18,7,78},
  {14,8,81},{12,9,83},{11,10,84},{12,11,83},{14,12,81},{17,13,78},
  {22,14,44},{50,14,73},{27,15,38},{56,15,68},
})
bands(b, 2, {
  {55,5,66},{29,6,40},{50,6,71},{23,7,44},{48,7,75},
  {19,8,37},{43,8,64},{16,9,32},{51,9,76},{22,10,45},{58,10,80},
})
bands(b, 3, {{57,4,63},{31,5,37},{52,5,58},{25,6,29},{68,6,72}})
bands(b, 4, {{15,12,29},{39,12,57},{67,12,81},{17,13,34},{47,13,63},{72,13,78},{22,14,44},{50,14,73},{27,15,38},{56,15,68}})

local c = blank(96, 24)
-- Detached wisps are short, irregular islands rather than a third bubble row.
bands(c, 1, {{6,8,19},{4,9,22},{7,10,18},{49,13,69},{45,14,73},{51,15,66}})
bands(c, 2, {{9,8,16},{7,9,13},{52,13,65},{49,14,58}})
bands(c, 3, {{10,7,14},{54,12,61}})
bands(c, 4, {{8,10,18},{51,15,66}})

local strip_a = indexed("cloud_strip_a", a)
local strip_b = indexed("cloud_strip_b", b)
local strip_c = indexed("cloud_strip_c", c)
local alpha_a = paintop.assert.binary_alpha {id = "alpha_a", image = strip_a.image}
local alpha_b = paintop.assert.binary_alpha {id = "alpha_b", image = strip_b.image}
local alpha_c = paintop.assert.binary_alpha {id = "alpha_c", image = strip_c.image}
local palette_a = paintop.assert.palette {
  id = "palette_a", image = strip_a.image, palette = palette(),
  transparent_rgb = "zero",
}
local palette_b = paintop.assert.palette {
  id = "palette_b", image = strip_b.image, palette = palette(),
  transparent_rgb = "zero",
}
local palette_c = paintop.assert.palette {
  id = "palette_c", image = strip_c.image, palette = palette(),
  transparent_rgb = "zero",
}

return paintop.plan {
  name = "ribbit-recoil-cloud-strips",
  description = "Three transparent asymmetric 96x24 Apollo64 cloud silhouettes with broken undersides and localized edge values.",
  policy = {
    resources = {max_nodes = 12, max_pixels_per_resource = 4096, max_splats = 0},
    execution = {deadline_ms = 30000, allowed_backends = {"cpu-reference"}},
  },
  nodes = {strip_a, strip_b, strip_c, alpha_a, alpha_b, alpha_c, palette_a, palette_b, palette_c},
  assertions = {
    {id = "alpha_a", subject = alpha_a.report},
    {id = "alpha_b", subject = alpha_b.report},
    {id = "alpha_c", subject = alpha_c.report},
    {id = "palette_a", subject = palette_a.report},
    {id = "palette_b", subject = palette_b.report},
    {id = "palette_c", subject = palette_c.report},
  },
  exports = {
    cloud_strip_a = {resource = strip_a.image, kind = "image", path = "cloud-strip-a.png", encoding = {format = "png"}},
    cloud_strip_b = {resource = strip_b.image, kind = "image", path = "cloud-strip-b.png", encoding = {format = "png"}},
    cloud_strip_c = {resource = strip_c.image, kind = "image", path = "cloud-strip-c.png", encoding = {format = "png"}},
  },
  evidence = {
    trace = "detailed", graph = {"dot"}, contact_sheet = true,
    materialize = {strip_a.image, strip_b.image, strip_c.image}, diffs = paintop.array {},
  },
}
