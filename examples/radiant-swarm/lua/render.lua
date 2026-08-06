local render = {}

local SW, SH = 192, 320

local function begin_frame(state)
  camera()
  clip()
  pal()
  palt()
  fillp()
  mosaic()
  rshift()

  cls(48)
  -- A restrained vertical-space backdrop. The lanes make dense radial bullet
  -- fields readable without competing with their color.
  rectfill(0, 0, SW - 1, SH - 1, 49)
  rectfill(0, 50, SW - 1, 319, 0)
  fillp(0x8888, 1)
  for y = 58, 310, 28 do rectfill(0, y, SW - 1, y + 1, 49) end
  fillp()
  for x = 12, SW - 1, 24 do
    line(x, 50, x - 20, 319, 1)
  end
  local pulse = state.frame % 96
  line(0, 80 + pulse * 2, SW - 1, 80 + pulse * 2, 2)
end

local function draw_stars(world)
  draw_tag("background")
  world:each({"pos", "star"}, function(_, pos, star)
    pset(pos.x, pos.y, star.color)
    if star.size > 1 then pset(pos.x, pos.y + 1, star.color) end
  end)
end

local function draw_pickups(world)
  draw_tag("pickups")
  world:each({"pos", "pickup"}, function(_, pos, pickup)
    local flash = (pickup.age % 12 < 6) and 31 or 39
    line(pos.x - 3, pos.y, pos.x + 3, pos.y, flash)
    line(pos.x, pos.y - 3, pos.x, pos.y + 3, flash)
    pset(pos.x, pos.y, 63)
  end)
end

local function draw_enemies(world)
  draw_tag("enemies")
  world:each({"pos", "enemy"}, function(_, pos, enemy)
    local color = enemy.boss and 45 or (enemy.kind == 2 and 38 or 13)
    local edge = enemy.hit_flash > 0 and 63 or color
    if enemy.boss then
      circfill(pos.x, pos.y, 15, 33)
      circ(pos.x, pos.y, 15, edge)
      circ(pos.x, pos.y, 10 + (enemy.age % 16) / 4, 47)
      line(pos.x - 18, pos.y, pos.x + 18, pos.y, 45)
      line(pos.x, pos.y - 18, pos.x, pos.y + 18, 45)
      circfill(pos.x, pos.y, 4, 63)
    elseif enemy.kind == 1 then
      circfill(pos.x, pos.y, 7, 9)
      circ(pos.x, pos.y, 8, edge)
      line(pos.x - 10, pos.y, pos.x + 10, pos.y, 14)
      pset(pos.x, pos.y, 63)
    elseif enemy.kind == 2 then
      rectfill(pos.x - 6, pos.y - 6, pos.x + 6, pos.y + 6, 25)
      rect(pos.x - 7, pos.y - 7, pos.x + 7, pos.y + 7, edge)
      line(pos.x - 9, pos.y - 9, pos.x + 9, pos.y + 9, 38)
      line(pos.x + 9, pos.y - 9, pos.x - 9, pos.y + 9, 38)
    else
      circfill(pos.x, pos.y, 6, 41)
      circ(pos.x, pos.y, 8, edge)
      circ(pos.x, pos.y, 3, 47)
    end
  end)
end

local function draw_bullets(world)
  draw_tag("bullets")
  world:each({"pos", "hostile"}, function(_, pos, bullet)
    local radius = bullet.radius
    if bullet.style == 2 then
      rectfill(pos.x - radius, pos.y - radius, pos.x + radius, pos.y + radius, bullet.color)
      pset(pos.x, pos.y, 63)
    elseif bullet.style == 3 then
      line(pos.x - radius - 1, pos.y, pos.x + radius + 1, pos.y, bullet.color)
      line(pos.x, pos.y - radius - 1, pos.x, pos.y + radius + 1, bullet.color)
      pset(pos.x, pos.y, 63)
    else
      circfill(pos.x, pos.y, radius, bullet.color)
      if radius > 1 then pset(pos.x, pos.y, 63) end
    end
    if bullet.grazed then pset(pos.x + radius + 1, pos.y, 7) end
  end)

  draw_tag("player_shots")
  world:each({"pos", "friendly"}, function(_, pos, shot)
    line(pos.x, pos.y + 4, pos.x, pos.y - 4, shot.color)
    pset(pos.x - 1, pos.y, 63)
    pset(pos.x + 1, pos.y, 63)
  end)
end

local function draw_particles(world)
  draw_tag("particles")
  world:each({"pos", "particle"}, function(_, pos, particle)
    if particle.size > 1 then
      circfill(pos.x, pos.y, particle.size, particle.color)
    else
      pset(pos.x, pos.y, particle.color)
    end
  end)
end

local function draw_player(world, state)
  draw_tag("player")
  world:each({"pos", "player"}, function(_, pos, player)
    if player.invulnerable > 0 and state.frame % 6 < 3 then return end
    local focused = btn(4)
    local body = focused and 7 or 6
    -- 17x19 luminous kite with an explicit 1px hit core.
    line(pos.x, pos.y - 10, pos.x - 8, pos.y + 7, body)
    line(pos.x, pos.y - 10, pos.x + 8, pos.y + 7, body)
    line(pos.x - 8, pos.y + 7, pos.x, pos.y + 3, 5)
    line(pos.x + 8, pos.y + 7, pos.x, pos.y + 3, 5)
    circfill(pos.x, pos.y, 3, 2)
    pset(pos.x, pos.y, 63)
    if focused then circ(pos.x, pos.y, 5, 14) end
  end)
end

local function draw_hud(state)
  draw_tag("ui")
  rectfill(0, 0, SW - 1, 21, 48)
  line(0, 21, SW - 1, 21, 3)
  print("SCORE " .. state.score, 4, 3, 63)
  print("GRAZE " .. state.graze, 4, 11, 7)
  print("HP " .. state.hp, 188, 3, state.hp > 1 and 14 or 38, "right")
  print("BOMB " .. state.bombs, 188, 11, 31, "right")
  if state.boss_hp > 0 then
    rectfill(24, 25, 167, 29, 33)
    local width = flr(142 * state.boss_hp / state.boss_max_hp)
    if width > 0 then rectfill(25, 26, 25 + width, 28, 45) end
    print("THE CHOIR", 96, 32, 47, "center")
  end
  if state.banner_frames > 0 then
    print(state.banner, 96, 45, state.banner_color, "center")
  end
end

local function title(state)
  draw_tag("ui")
  fillp(0x5a5a, 42)
  circfill(96, 125, 48, 41)
  fillp()
  circ(96, 125, 49, 47)
  circ(96, 125, 34 + (state.frame % 20) / 4, 45)
  for spoke = 0, 7 do
    local x = 96 + 58 * cos(spoke / 8)
    local y = 125 + 58 * sin(spoke / 8)
    line(96, 125, x, y, 43)
  end
  print("RADIANT", 96, 94, 63, "center")
  print("SWARM", 96, 108, 31, "center")
  print("A  FIRE / FOCUS", 96, 205, 7, "center")
  print("B  NOVA BOMB", 96, 217, 31, "center")
  print("D-PAD  MOVE", 96, 229, 14, "center")
  if state.frame % 60 < 42 then print("PRESS A", 96, 266, 63, "center") end
  print("BUILT ON THE LUA ECS PROTOTYPE", 96, 292, 5, "center")
end

local function ending(state, won)
  draw_tag("ui")
  rectfill(18, 104, 173, 220, 48)
  rect(18, 104, 173, 220, won and 31 or 38)
  print(won and "SWARM PACIFIED" or "SIGNAL LOST", 96, 122, won and 31 or 38, "center")
  print("SCORE " .. state.score, 96, 151, 63, "center")
  print("GRAZE " .. state.graze, 96, 163, 7, "center")
  print("PEAK ENTITIES " .. state.peak_alive, 96, 175, 14, "center")
  if state.frame % 60 < 42 then print("A  FLY AGAIN", 96, 203, 63, "center") end
end

function render.draw(world, state)
  begin_frame(state)
  draw_stars(world)
  if state.phase == "title" then
    title(state)
  else
    draw_pickups(world)
    draw_enemies(world)
    draw_bullets(world)
    draw_particles(world)
    draw_player(world, state)
    draw_hud(state)
    if state.phase == "gameover" then ending(state, false) end
    if state.phase == "victory" then ending(state, true) end
  end
  draw_tag()
end

return render
