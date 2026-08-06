local game = require("game")

function _init()
  game.init()
end

function _update()
  game.update()
end

function _draw()
  game.draw()
end

function dev_status()
  return game.status()
end

function dev_start()
  game.start()
end

function dev_stress()
  game.stress()
end

function dev_damage_boss(amount)
  game.damage_boss(amount)
end
