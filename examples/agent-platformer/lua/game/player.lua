local player = {
  x = 32,
  y = 279,
  vx = 0,
  vy = 0,
  grounded = true,
  facing_left = false,
  steps = 0,
  jumps = 0,
}

local ground_y = 279

function player.reset()
  player.x = 32
  player.y = ground_y
  player.vx = 0
  player.vy = 0
  player.grounded = true
  player.facing_left = false
  player.steps = 0
  player.jumps = 0
end

function player.update()
  local move = 0
  if btn(0) then move = move - 1 end
  if btn(1) then move = move + 1 end
  player.vx = move * 1.5
  if move ~= 0 then
    player.x = mid(4, player.x + player.vx, 187)
    player.facing_left = move < 0
    player.steps = player.steps + 1
  end

  if btnp(4) and player.grounded then
    player.vy = -5.5
    player.grounded = false
    player.jumps = player.jumps + 1
    sfx(1)
  end

  if not player.grounded then
    player.vy = player.vy + 0.32
    player.y = player.y + player.vy
    if player.y >= ground_y then
      player.y = ground_y
      player.vy = 0
      player.grounded = true
    end
  end
end

function player.draw()
  circfill(flr(player.x), 281, 5, 9)
  local animation = player.vx == 0 and "player.idle" or "player.walk"
  aspr(animation, flr(player.x), flr(player.y), 0, player.facing_left, false)
end

return player
