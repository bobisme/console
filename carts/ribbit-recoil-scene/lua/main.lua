local tile_classes = require("generated.tile_classes")
local layers = require("generated.decorative_layers")
local objects = require("generated.objects")

function _draw()
  cls(48)
  layers.draw_visible("far_girders", 0, 0, 16, 8)
  map(0, 0, 0, 0, 16, 8)
end

function dev_scene_status()
  return {
    solid = tile_classes.is_solid(mget(0, 0)),
    hazard = tile_classes.is_hazard(mget(4, 0)),
    objects = #objects,
  }
end
