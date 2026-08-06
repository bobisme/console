local render = {}
local SW, SH = 192, 320

local function clamp(value, low, high)
  if value < low then return low end
  if value > high then return high end
  return value
end

local function reset_draw_state()
  camera()
  clip()
  pal()
  palt()
  fillp()
  mosaic()
  rshift()
end

local function draw_title(state)
  draw_tag("title")
  cls(17)
  rectfill(0, 0, SW - 1, SH - 1, 18)
  fillp(0x8888, 19)
  for y = 0, SH - 1, 16 do rectfill(0, y, SW - 1, y + 7, 17) end
  fillp()
  local moon_y = 90 + sin(state.frame / 180) * 5
  fillp(0x5a5a, 30)
  circfill(96, moon_y, 48, 29)
  fillp()
  circ(96, moon_y, 49, 31)
  for root = 0, 11 do
    local turn = root / 12 + state.frame / 1200
    line(96, moon_y, 96 + cos(turn) * 72, moon_y + sin(turn) * 72, 22 + root % 3)
  end
  print("MIRELIGHT", 96, 72, 63, "center")
  print("SURVIVORS", 96, 86, 31, "center")
  print("THE GARDEN REMEMBERS", 96, 151, 31, "center")
  print("D-PAD  MOVE", 96, 185, 63, "center")
  print("A  DASH", 96, 197, 46, "center")
  print("B  ROOT PULSE", 96, 209, 31, "center")
  local start_label = state.dense_selected and "A  BEGIN DENSE HUNT" or "A  BEGIN HUNT"
  if state.frame % 60 < 44 then print(start_label, 96, 246, state.dense_selected and 38 or 63, "center") end
  print("B  DENSE LOAD: " .. (state.dense_selected and "ON" or "OFF"), 96, 263, state.dense_selected and 38 or 46, "center")
  print("800+ CREATURE CHALLENGE", 96, 276, 31, "center")
  print("AUTO-FIRE SEEKS THE NEAREST ROOT", 96, 302, 46, "center")
end

local function enemy_sprite(enemy)
  if enemy.kind == 2 then return 7 end
  if enemy.kind == 3 then return 8 end
  if enemy.kind == 4 then return 12 end
  return 6
end

local function draw_world(world, state, player_id, arena_w, arena_h)
  local player_pos = world:get(player_id, "pos")
  local cam_x = clamp(flr(player_pos.x - SW / 2), 0, arena_w - SW)
  local cam_y = clamp(flr(player_pos.y - SH / 2), 0, arena_h - SH)

  draw_tag("background")
  cls(17)
  camera(cam_x, cam_y)
  map(0, 0, 0, 0, 48, 64)
  fillp(0x8888, 19)
  for x = 0, arena_w, 64 do line(x, 0, x, arena_h - 1, 18) end
  for y = 0, arena_h, 64 do line(0, y, arena_w - 1, y, 18) end
  fillp()

  draw_tag("entities")
  world:each({"pos"}, function(id, pos)
    if pos.x >= cam_x - 10 and pos.x <= cam_x + SW + 10 and pos.y >= cam_y - 10 and pos.y <= cam_y + SH + 10 then
      local enemy = world:get(id, "enemy")
      if enemy then
        local tile = enemy_sprite(enemy)
        local flip = (id + flr(state.frame / 12)) % 2 == 0
        spr(tile, pos.x - 4, pos.y - 4, 1, 1, flip, false)
        if enemy.hit_flash > 0 then circ(pos.x, pos.y, enemy.radius + 2, 63) end
        if enemy.elite then
          circ(pos.x, pos.y, enemy.radius + 3 + state.frame % 3, 31)
          pset(pos.x, pos.y, 63)
        end
      else
        local projectile = world:get(id, "projectile")
        if projectile then
          line(pos.x - projectile.vx, pos.y - projectile.vy, pos.x, pos.y, projectile.kind == "thorn" and 46 or 45)
          pset(pos.x, pos.y, 63)
        else
          local pickup = world:get(id, "pickup")
          if pickup then
            spr(10, pos.x - 4, pos.y - 4)
            if pickup.age % 20 < 6 then circ(pos.x, pos.y, 5, 31) end
          else
            local particle = world:get(id, "particle")
            if particle then
              pset(pos.x, pos.y, particle.color)
              if particle.ttl % 7 == 0 then pset(pos.x + 1, pos.y, 63) end
            elseif id == player_id then
              local player = world:get(id, "player")
              if player.invulnerability == 0 or state.frame % 6 < 3 then
                spr(state.dash_flash > 0 and 11 or 5, pos.x - 4, pos.y - 4)
                circ(pos.x, pos.y, 5, state.mode == "stress" and 38 or 46)
                pset(pos.x, pos.y, 63)
              end
            end
          end
        end
      end
    end
  end)

  draw_tag("feedback")
  if state.pulse_flash > 0 then
    local radius = (24 - state.pulse_flash) * 3
    circ(player_pos.x, player_pos.y, radius, state.pulse_flash % 4 < 2 and 31 or 29)
  end
  if state.dash_flash > 0 then
    circ(player_pos.x, player_pos.y, 8 + (12 - state.dash_flash) * 2, 46)
  end

  camera()
  draw_tag("ui")
  rectfill(0, 0, SW - 1, 18, 16)
  line(0, 18, SW - 1, 18, state.mode == "stress" and 38 or 29)
  print("HP " .. state.hp, 4, 3, state.hp > 2 and 46 or 38)
  print("LV " .. state.level, 4, 10, 31)
  print("SCORE " .. state.score, 188, 3, 63, "right")
  print("LIVE " .. state.alive, 188, 10, state.alive >= 800 and 38 or 45, "right")
  local xp_width = flr(84 * state.xp / max(1, state.next_xp))
  rectfill(54, 4, 138, 8, 19)
  if xp_width > 0 then rectfill(54, 4, 53 + xp_width, 8, 30) end
  local seconds = max(0, flr((4500 - state.frame) / 60))
  local center_label = state.mode == "stress" and "DENSE" or (state.finale and "ROOT-MOON" or (seconds .. "S"))
  print(center_label, 96, 11, state.mode == "stress" and 38 or (state.finale and 38 or 28), "center")
  if state.banner_frames > 0 then
    rectfill(18, 27, 173, 39, 16)
    rect(18, 27, 173, 39, state.mode == "stress" and 38 or 29)
    print(state.banner, 96, 31, 63, "center")
  end
  if state.damage_flash > 0 then
    rect(1, 20, SW - 2, SH - 2, state.damage_flash % 4 < 2 and 38 or 63)
  end
end

local function draw_levelup(state)
  draw_tag("ui")
  fillp(0x8888, 16)
  rectfill(0, 20, SW - 1, SH - 1, 19)
  fillp()
  rectfill(12, 76, 179, 244, 16)
  rect(12, 76, 179, 244, 31)
  print("THE GARDEN ANSWERS", 96, 91, 63, "center")
  print("LEVEL " .. state.level, 96, 105, 31, "center")
  local choices = {"DEEPER THORNS", "QUICKER SEEDS", "HOMING DEW"}
  for index = 1, 3 do
    local y = 139 + (index - 1) * 31
    local selected = index == state.level_selection
    if selected then
      rectfill(25, y - 5, 166, y + 15, 22)
      rect(25, y - 5, 166, y + 15, 46)
    end
    print((selected and "> " or "  ") .. choices[index], 96, y, selected and 63 or 28, "center")
  end
  print("UP/DOWN CHOOSE  A ACCEPT", 96, 225, 45, "center")
end

local function draw_ending(state, victory)
  draw_tag("ui")
  fillp(0x5a5a, victory and 29 or 38)
  rectfill(20, 92, 171, 229, 16)
  fillp()
  rect(20, 92, 171, 229, victory and 31 or 38)
  print(victory and "DAWN TAKES ROOT" or "THE MIRE CLOSES", 96, 111, victory and 63 or 38, "center")
  print("SCORE " .. state.score, 96, 143, 46, "center")
  print("LEVEL " .. state.level, 96, 157, 31, "center")
  print("CREATURES " .. state.kills, 96, 171, 29, "center")
  print("A  NEW HUNT", 96, 201, 63, "center")
  print("B  DENSITY GARDEN", 96, 214, 38, "center")
end

function render.draw(world, state, player_id, arena_w, arena_h)
  reset_draw_state()
  if state.phase == "title" then
    draw_title(state)
    return
  end
  draw_world(world, state, player_id, arena_w, arena_h)
  if state.phase == "levelup" then draw_levelup(state) end
  if state.phase == "gameover" then draw_ending(state, false) end
  if state.phase == "victory" then draw_ending(state, true) end
  draw_tag()
end

return render
