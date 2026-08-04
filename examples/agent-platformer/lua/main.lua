local world = require("game.world")
local hud = require("ui.hud")

function _init()
  srand(7)
  world.init()
  music(0)
end

function _update()
  world.update()
end

function _draw()
  cls(1)
  for y=0,279,8 do
    local color = 1 + flr(y / 56)
    rectfill(0, y, 191, y + 7, color)
  end
  world.draw()
  hud.draw(world.status())
end

function dev_status()
  return world.status()
end
