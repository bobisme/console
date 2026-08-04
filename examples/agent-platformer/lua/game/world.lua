local player = require("game.player")

local world = {
  score = 0,
  moth_x = 142,
  moth_y = 244,
  moth_visible = true,
  respawn = 0,
}

function world.init()
  player.reset()
  world.score = 0
  world.moth_x = 142
  world.moth_y = 244
  world.moth_visible = true
  world.respawn = 0
end

function world.update()
  player.update()
  world.moth_y = 244 + sin(t() * 1.5) * 8

  if world.moth_visible and abs(player.x - world.moth_x) < 12 and abs(player.y - world.moth_y) < 18 then
    world.score = world.score + 1
    world.moth_visible = false
    world.respawn = 60
    sfx(2)
  elseif not world.moth_visible then
    world.respawn = world.respawn - 1
    if world.respawn <= 0 then
      world.moth_x = 32 + rnd(128)
      world.moth_visible = true
    end
  end
end

function world.draw()
  for x=12,188,32 do
    circfill(x, 72 + sin((x + t() * 60) / 80) * 3, 1, 7)
  end
  rectfill(0, 288, 191, 319, 8)
  map(0, 0, 0, 280, 24, 1)
  if world.moth_visible then
    aspr("moth.hover", flr(world.moth_x), flr(world.moth_y))
  else
    for i=0,5 do
      local angle = i / 6
      pset(world.moth_x + cos(angle) * (60 - world.respawn) / 5,
           world.moth_y + sin(angle) * (60 - world.respawn) / 5, 31 + i)
    end
  end
  player.draw()
end

function world.status()
  return {
    x = player.x,
    y = player.y,
    grounded = player.grounded,
    steps = player.steps,
    jumps = player.jumps,
    score = world.score,
    moth_visible = world.moth_visible,
  }
end

return world
