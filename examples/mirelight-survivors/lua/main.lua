local game = require("game")

devhook.register("status", {
  description="Return bounded semantic state and stress telemetry",
  phase="post_frame",
  run=function(_args) return game.status() end,
})

devhook.register("start", {
  description="Start a normal survivors run before frame one",
  phase="pre_frame",
  run=function(_args) game.start(false) end,
})

devhook.register("stress", {
  description="Start the deterministic invulnerable 800-enemy challenge",
  phase="pre_frame",
  run=function(_args) game.start(true) end,
})

devhook.register("grant_xp", {
  description="Award scalar XP through the ordinary progression path",
  phase="post_frame",
  run=function(amount) game.grant_xp(amount) end,
})

devhook.register("damage_player", {
  description="Apply scalar damage through the ordinary player damage path",
  phase="post_frame",
  run=function(amount) game.damage_player(amount) end,
})

devhook.register("enter_finale", {
  description="Enter the ordinary final-wave transition",
  phase="post_frame",
  run=function(_args) game.enter_finale() end,
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
