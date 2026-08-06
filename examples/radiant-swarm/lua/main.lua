local game = require("game")

devhook.register("status", {
  description = "Return bounded semantic state for deterministic inspection",
  phase = "post_frame",
  run = function(_args)
    return game.status()
  end,
})

devhook.register("start", {
  description = "Start the standard encounter before the first frame",
  phase = "pre_frame",
  run = function(_args)
    game.start()
  end,
})

devhook.register("stress", {
  description = "Start the deterministic invulnerable entity stress encounter",
  phase = "pre_frame",
  run = function(_args)
    game.stress()
  end,
})

devhook.register("damage_boss", {
  description = "Apply a scalar amount of deterministic boss damage",
  phase = "post_frame",
  run = function(amount)
    game.damage_boss(amount)
  end,
})

function _init()
  game.init()
end

function _update()
  game.update()
end

function _draw()
  game.draw()
end
