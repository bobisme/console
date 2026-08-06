local patterns = require("patterns")
local render = require("render")

local game = {}
local world = ecs.world("arena", {capacity=1200})
local WORLD_LIMIT = 1180
local SW, SH = 192, 320

local state = {
  phase = "title",
  frame = 0,
  score = 0,
  graze = 0,
  hp = 3,
  bombs = 3,
  wave = 0,
  banner = "",
  banner_frames = 0,
  banner_color = 31,
  boss_hp = 0,
  boss_max_hp = 0,
  boss_phase = 0,
  boss_transition = 0,
  formation = "",
  graze_chain = 0,
  graze_timer = 0,
  graze_flash = 0,
  pickup_flash = 0,
  nova_flash = 0,
  damage_flash = 0,
  peak_alive = 0,
  peak_bullets = 0,
  dropped_spawns = 0,
  debug_invulnerable = false,
}

local player_id = nil
local enemy_serial = 0
local boss_id = nil

local function safe_spawn(components)
  if world:count() >= WORLD_LIMIT then
    state.dropped_spawns = state.dropped_spawns + 1
    return nil
  end
  return world:spawn(components)
end

local function spawn_star(index)
  local depth = 1 + (index % 3)
  safe_spawn({
    pos = {x=rnd(SW), y=22 + rnd(SH - 22)},
    vel = {x=0, y=0.08 + depth * 0.055},
    star = {color=depth == 3 and 6 or (depth == 2 and 3 or 2), size=depth == 3 and 2 or 1},
  })
end

local function seed_stars(count)
  for index = 1, count do spawn_star(index) end
end

local function reset_state(phase)
  world:clear()
  state.phase = phase
  state.frame = 0
  state.score = 0
  state.graze = 0
  state.hp = 3
  state.bombs = 3
  state.wave = 0
  state.banner = ""
  state.banner_frames = 0
  state.banner_color = 31
  state.boss_hp = 0
  state.boss_max_hp = 0
  state.boss_phase = 0
  state.boss_transition = 0
  state.formation = ""
  state.graze_chain = 0
  state.graze_timer = 0
  state.graze_flash = 0
  state.pickup_flash = 0
  state.nova_flash = 0
  state.damage_flash = 0
  state.peak_alive = 0
  state.peak_bullets = 0
  state.dropped_spawns = 0
  state.debug_invulnerable = false
  player_id = nil
  boss_id = nil
  enemy_serial = 0
  seed_stars(84)
end

local function spawn_player()
  player_id = safe_spawn({
    pos = {x=96, y=282},
    player = {invulnerable=75},
    radius = 2,
  })
end

local function particle(x, y, color, speed, life, size)
  local angle = flr(rnd(24))
  local vx = speed * cos(angle / 24)
  local vy = speed * sin(angle / 24)
  return safe_spawn({
    pos = {x=x, y=y},
    vel = {x=vx, y=vy},
    particle = {color=color, size=size or 1},
    life = {frames=life},
  })
end

local function burst(x, y, color, count, speed, life)
  for index = 1, count do
    particle(x, y, index % 3 == 0 and 63 or color, speed * (0.5 + rnd(0.7)), life + flr(rnd(12)), index % 7 == 0 and 2 or 1)
  end
end

local function hostile_bullet(x, y, vx, vy, style)
  style = style or {}
  return safe_spawn({
    pos = {x=x, y=y},
    vel = {x=vx, y=vy},
    bullet = true,
    hostile = {
      kind=style.kind or "orb",
      radius=style.radius or 2,
      color=style.color or 45,
      style=style.style or 1,
      grazed=false,
    },
    life = {frames=style.life or 420},
  })
end

local function friendly_bullet(x, y, vx, vy, damage, color)
  return safe_spawn({
    pos = {x=x, y=y},
    vel = {x=vx, y=vy},
    bullet = true,
    friendly = {damage=damage or 1, color=color or 14},
    life = {frames=90},
  })
end

local function spawn_pickup(x, y)
  safe_spawn({
    pos = {x=x, y=y},
    vel = {x=0, y=0.55},
    pickup = {age=0},
    life = {frames=420},
  })
end

local function spawn_enemy(kind, x, y, fixed)
  enemy_serial = enemy_serial + 1
  local hp = kind == 3 and 12 or (kind == 2 and 9 or 6)
  return safe_spawn({
    pos = {x=x, y=y or 37},
    enemy = {
      kind=kind,
      hp=hp,
      max_hp=hp,
      age=0,
      base_x=x,
      serial=enemy_serial,
      fixed=fixed or false,
      boss=false,
      dead=false,
      hit_flash=0,
      cue=0,
      radius=kind == 2 and 8 or 7,
    },
  })
end

local function spawn_boss()
  -- End the wave sentence before the boss begins its own. Carrying ordinary
  -- emitters into the warning beat made the phase change noisy and unfair.
  world:each({"enemy"}, function(id) world:despawn(id) end)
  world:each({"hostile"}, function(id) world:despawn(id) end)
  local hp = 240
  boss_id = safe_spawn({
    pos = {x=96, y=77},
    enemy = {
      kind=4,
      hp=hp,
      max_hp=hp,
      age=0,
      base_x=96,
      serial=999,
      fixed=true,
      boss=true,
      dead=false,
      hit_flash=0,
      cue=0,
      radius=16,
    },
  })
  state.boss_hp = hp
  state.boss_max_hp = hp
  state.boss_phase = 1
  state.boss_transition = 120
  state.banner = "WARNING: THE CHOIR"
  state.banner_color = 47
  state.banner_frames = 180
  sfx(5, 5)
end

local function player_position()
  if player_id == nil then return nil, nil end
  return world:get(player_id, "pos"), world:get(player_id, "player")
end

local function update_motion()
  world:each({"pos", "vel"}, function(_, pos, vel)
    pos.x = pos.x + vel.x
    pos.y = pos.y + vel.y
  end)
  world:each({"pos", "star"}, function(_, pos)
    if pos.y >= SH then
      pos.y = 23
      pos.x = (pos.x * 73 + 41) % SW
    end
  end)
  world:each({"vel", "particle"}, function(_, vel)
    vel.x = vel.x * 0.97
    vel.y = vel.y * 0.97
  end)
end

local function update_lifetimes_and_bounds()
  world:each({"life"}, function(id, life)
    life.frames = life.frames - 1
    if life.frames <= 0 then world:despawn(id) end
  end)
  world:each({"pos", "bullet"}, function(id, pos)
    if pos.x < -24 or pos.x > SW + 24 or pos.y < -32 or pos.y > SH + 32 then
      world:despawn(id)
    end
  end)
end

local function move_and_fire_player()
  local pos, player = player_position()
  if pos == nil then return end
  if player.invulnerable > 0 then player.invulnerable = player.invulnerable - 1 end
  local dx = (btn(1) and 1 or 0) - (btn(0) and 1 or 0)
  local dy = (btn(3) and 1 or 0) - (btn(2) and 1 or 0)
  local speed = btn(4) and 1.25 or 2.35
  if dx ~= 0 and dy ~= 0 then speed = speed * 0.70710678 end
  pos.x = mid(9, pos.x + dx * speed, SW - 10)
  pos.y = mid(52, pos.y + dy * speed, SH - 12)

  if btn(4) and state.frame % 4 == 0 then
    friendly_bullet(pos.x - 4, pos.y - 9, -0.12, -5.4, 1, 14)
    friendly_bullet(pos.x + 4, pos.y - 9, 0.12, -5.4, 1, 31)
    if state.frame % 8 == 0 then sfx(0, 4) end
  end
end

local function clear_hostile_bullets(color)
  local cleared = 0
  world:each({"pos", "hostile"}, function(id, pos)
    cleared = cleared + 1
    if cleared % 9 == 0 then particle(pos.x, pos.y, color or 31, 0.7, 20, 1) end
    world:despawn(id)
  end)
  return cleared
end

local function enter_boss_phase(phase, pos)
  state.boss_phase = phase
  state.boss_transition = 90
  state.banner = phase == 2 and "CHOIR: COUNTERPOINT" or "CHOIR: FINALE"
  state.banner_color = phase == 2 and 31 or 47
  state.banner_frames = 110
  clear_hostile_bullets(phase == 2 and 31 or 47)
  burst(pos.x, pos.y, phase == 2 and 31 or 47, 30, 1.8, 45)
  sfx(8, 5)
end

local function update_boss_phase(pos, enemy)
  local ratio = enemy.hp / enemy.max_hp
  if state.boss_phase == 1 and ratio <= 0.67 then
    enter_boss_phase(2, pos)
  elseif state.boss_phase == 2 and ratio <= 0.34 then
    enter_boss_phase(3, pos)
  end
end

local function fire_enemy(pos, enemy, player_pos)
  if enemy.age < 35 or player_pos == nil then return end
  enemy.cue = max(0, enemy.cue - 1)
  if enemy.boss then
    if state.boss_transition > 0 then return end
    if state.boss_phase == 1 then
      if enemy.age % 12 == 0 then
        patterns.spiral(hostile_bullet, pos.x, pos.y, flr(enemy.age / 4), 1.08, {kind="spiral",radius=2,color=45,style=1})
      end
      if enemy.age % 64 == 52 then enemy.cue = 12 end
      if enemy.age % 64 == 0 then
        patterns.ring(hostile_bullet, pos.x, pos.y, 16, 1.02, flr(enemy.age / 16), {kind="ring",radius=2,color=31,style=2})
      end
    elseif state.boss_phase == 2 then
      if enemy.age % 9 == 0 then
        patterns.petal(hostile_bullet, pos.x, pos.y, flr(enemy.age / 5), 5, 2, 1.28, {kind="petal",radius=2,color=31,style=3})
      end
      if enemy.age % 58 == 46 then enemy.cue = 12 end
      if enemy.age % 58 == 0 then
        patterns.aimed(hostile_bullet, pos.x, pos.y, player_pos.x, player_pos.y, 7, 1.58, 0.105, {kind="fan",radius=2,color=47,style=3})
      end
    else
      if enemy.age % 7 == 0 then
        patterns.spiral(hostile_bullet, pos.x, pos.y, flr(enemy.age / 2), 1.32, {kind="spiral",radius=2,color=47,style=1})
      end
      if enemy.age % 43 == 31 then enemy.cue = 12 end
      if enemy.age % 43 == 0 then
        patterns.ring(hostile_bullet, pos.x, pos.y, 16, 1.22, flr(enemy.age / 11), {kind="ring",radius=2,color=31,style=2})
      end
      if enemy.age % 71 == 0 then
        patterns.aimed(hostile_bullet, pos.x, pos.y, player_pos.x, player_pos.y, 5, 1.82, 0.13, {kind="fan",radius=2,color=38,style=3})
      end
    end
  elseif enemy.kind == 1 then
    if enemy.age % 48 == 38 then enemy.cue = 10 end
    if enemy.age % 48 == 0 then
      patterns.aimed(hostile_bullet, pos.x, pos.y, player_pos.x, player_pos.y, 5, 1.5, 0.13, {kind="fan",radius=2,color=38,style=3})
    end
  elseif enemy.kind == 2 then
    if enemy.age % 67 == 55 then enemy.cue = 12 end
    if enemy.age % 67 == 0 then
      patterns.ring(hostile_bullet, pos.x, pos.y, 12, 1.0, enemy.serial * 2 + flr(enemy.age / 20), {kind="ring",radius=2,color=31,style=2})
    end
  elseif enemy.kind == 3 then
    if enemy.age % 11 == 0 then
      patterns.spiral(hostile_bullet, pos.x, pos.y, enemy.serial * 3 + flr(enemy.age / 5), 1.22, {kind="spiral",radius=2,color=45,style=1})
    end
    if enemy.age % 96 == 0 then
      patterns.ring(hostile_bullet, pos.x, pos.y, 8, 0.82, enemy.serial, {kind="halo",radius=3,color=13,style=1})
    end
    if enemy.age % 96 == 84 then enemy.cue = 12 end
  end
end

local function update_enemies()
  local player_pos = player_position()
  world:each({"pos", "enemy"}, function(id, pos, enemy)
    enemy.age = enemy.age + 1
    if enemy.hit_flash > 0 then enemy.hit_flash = enemy.hit_flash - 1 end
    if enemy.boss then
      update_boss_phase(pos, enemy)
      pos.x = 96 + 48 * sin(enemy.age / 310)
      pos.y = 76 + 8 * sin(enemy.age / 97)
    elseif enemy.fixed then
      pos.y = 52 + (enemy.serial % 3) * 22
      pos.x = enemy.base_x + 8 * sin((enemy.age + enemy.serial * 11) / 120)
    elseif enemy.age < 55 then
      pos.y = pos.y + 0.7
    else
      pos.x = enemy.base_x + (16 + enemy.kind * 3) * sin((enemy.age + enemy.serial * 19) / (150 - enemy.kind * 12))
      pos.y = 73 + enemy.kind * 15 + 8 * sin((enemy.age + enemy.serial * 7) / 210)
    end
    fire_enemy(pos, enemy, player_pos)
    if not enemy.boss and not enemy.fixed and enemy.age > 680 then world:despawn(id) end
  end)
end

local formation_names = {"LANCER WEDGE", "TWIN GATES", "WEAVING LINE", "SPIRAL CHOIR"}

local function spawn_formation(index)
  local formation = ((index - 1) % #formation_names) + 1
  state.formation = formation_names[formation]
  if formation == 1 then
    spawn_enemy(1, 96, 24)
    spawn_enemy(1, 68, 10)
    spawn_enemy(1, 124, 10)
  elseif formation == 2 then
    spawn_enemy(2, 42, 18)
    spawn_enemy(2, 150, 18)
  elseif formation == 3 then
    for slot = 1, 4 do spawn_enemy(1, 24 + slot * 29, 8 - (slot % 2) * 16) end
  else
    spawn_enemy(3, 96, 16)
    spawn_enemy(1, 50, 4)
    spawn_enemy(1, 142, 4)
  end
end

local function spawn_waves()
  if state.frame < 900 then
    local interval = state.frame < 360 and 118 or (state.frame < 660 and 96 or 82)
    if state.frame % interval == 1 then
      state.wave = state.wave + 1
      spawn_formation(state.wave)
      state.banner = state.formation
      state.banner_color = state.wave % 4 == 0 and 45 or 14
      state.banner_frames = 58
    end
  elseif state.frame == 900 then
    spawn_boss()
  end
  if state.frame > 420 and state.frame % 180 == 0 and boss_id == nil then
    patterns.rain(hostile_bullet, state.frame, 9, 0.95, {kind="rain",radius=2,color=13,style=2})
  end
end

local function kill_enemy(id, pos, enemy)
  if enemy.dead then return end
  enemy.dead = true
  world:despawn(id)
  local reward = enemy.boss and 25000 or (500 + enemy.kind * 250)
  state.score = state.score + reward
  burst(pos.x, pos.y, enemy.boss and 47 or 31, enemy.boss and 48 or 14, enemy.boss and 2.4 or 1.5, enemy.boss and 80 or 35)
  sfx(enemy.boss and 4 or 1, 5)
  if enemy.boss then
    boss_id = nil
    state.boss_hp = 0
    state.phase = "victory"
    state.boss_phase = 0
    state.frame = 0
    state.banner_frames = 0
    music(-1)
    world:each({"hostile"}, function(bullet_id) world:despawn(bullet_id) end)
  elseif enemy.serial % 4 == 0 then
    spawn_pickup(pos.x, pos.y)
  end
end

local function collide_player_shots()
  local enemies = {}
  world:each({"pos", "enemy"}, function(id, pos, enemy)
    if not enemy.dead then enemies[#enemies + 1] = {id=id,pos=pos,enemy=enemy} end
  end)
  world:each({"pos", "friendly"}, function(shot_id, shot_pos, shot)
    for index = 1, #enemies do
      local target = enemies[index]
      if not target.enemy.dead then
        local dx, dy = shot_pos.x - target.pos.x, shot_pos.y - target.pos.y
        local radius = target.enemy.radius + 2
        if dx * dx + dy * dy <= radius * radius then
          target.enemy.hp = target.enemy.hp - shot.damage
          target.enemy.hit_flash = 3
          world:despawn(shot_id)
          if target.enemy.boss then state.boss_hp = max(0, target.enemy.hp) end
          if target.enemy.hp <= 0 then
            kill_enemy(target.id, target.pos, target.enemy)
          elseif state.frame % 3 == 0 then
            particle(shot_pos.x, shot_pos.y, 63, 0.8, 12, 1)
          end
          break
        end
      end
    end
  end)
end

local function damage_player(pos, player)
  if state.debug_invulnerable or player.invulnerable > 0 then return end
  state.hp = state.hp - 1
  player.invulnerable = 105
  burst(pos.x, pos.y, 38, 22, 1.7, 38)
  state.damage_flash = 24
  state.graze_chain = 0
  state.graze_timer = 0
  sfx(9, 5)
  if state.hp <= 0 then
    state.phase = "gameover"
    state.frame = 0
    music(-1)
    sfx(4, 5)
  else
    state.banner = "SIGNAL FRACTURED"
    state.banner_color = 38
    state.banner_frames = 75
  end
end

local function collide_hostile_bullets()
  local player_pos, player = player_position()
  if player_pos == nil then return end
  world:each({"pos", "hostile"}, function(id, pos, bullet)
    local dx, dy = pos.x - player_pos.x, pos.y - player_pos.y
    local distance = dx * dx + dy * dy
    local hit_radius = bullet.radius + 2
    local graze_radius = bullet.radius + 12
    if distance <= hit_radius * hit_radius then
      if not state.debug_invulnerable and player.invulnerable <= 0 then
        world:despawn(id)
        damage_player(player_pos, player)
      end
    elseif distance <= graze_radius * graze_radius and not bullet.grazed then
      bullet.grazed = true
      state.graze = state.graze + 1
      state.graze_chain = state.graze_chain + 1
      state.graze_timer = 90
      state.graze_flash = 12
      state.score = state.score + 25 + min(20, state.graze_chain) * 5
      particle(pos.x, pos.y, 7, 0.35, 12, 1)
      if state.graze_chain % 4 == 1 then sfx(6, 5) end
    end
  end)
end

local function update_pickups()
  local player_pos = player_position()
  if player_pos == nil then return end
  world:each({"pos", "pickup"}, function(id, pos, pickup)
    pickup.age = pickup.age + 1
    local dx, dy = player_pos.x - pos.x, player_pos.y - pos.y
    local distance = dx * dx + dy * dy
    if distance < 48 * 48 then
      local length = math.sqrt(distance)
      if length > 0 then
        pos.x = pos.x + dx / length * 1.4
        pos.y = pos.y + dy / length * 1.4
      end
    end
    if distance < 8 * 8 then
      world:despawn(id)
      state.score = state.score + 1200
      state.bombs = min(3, state.bombs + 1)
      state.pickup_flash = 36
      burst(pos.x, pos.y, 39, 10, 1.1, 24)
      state.banner = "NOVA CHARGE +1"
      state.banner_color = 39
      state.banner_frames = 45
      sfx(7, 5)
    end
  end)
end

local function nova_bomb()
  if state.bombs <= 0 then return end
  state.bombs = state.bombs - 1
  local player_pos = player_position()
  local cleared = 0
  world:each({"pos", "hostile"}, function(id, pos)
    cleared = cleared + 1
    if cleared % 7 == 0 then particle(pos.x, pos.y, 31, 1.1, 28, 1) end
    world:despawn(id)
  end)
  world:each({"pos", "enemy"}, function(id, pos, enemy)
    enemy.hp = enemy.hp - 14
    enemy.hit_flash = 8
    if enemy.boss then state.boss_hp = max(0, enemy.hp) end
    if enemy.hp <= 0 then kill_enemy(id, pos, enemy) end
  end)
  state.score = state.score + cleared * 10
  state.nova_flash = 36
  state.banner = "NOVA " .. cleared
  state.banner_color = 31
  state.banner_frames = 75
  sfx(2, 4)
end

local function update_feedback()
  if state.graze_timer > 0 then
    state.graze_timer = state.graze_timer - 1
  elseif state.graze_chain > 0 then
    state.graze_chain = 0
  end
  if state.graze_flash > 0 then state.graze_flash = state.graze_flash - 1 end
  if state.pickup_flash > 0 then state.pickup_flash = state.pickup_flash - 1 end
  if state.nova_flash > 0 then state.nova_flash = state.nova_flash - 1 end
  if state.damage_flash > 0 then state.damage_flash = state.damage_flash - 1 end
  if state.boss_transition > 0 then state.boss_transition = state.boss_transition - 1 end
end

local function update_peak()
  local alive = world:count()
  local bullets = world:count({"hostile"})
  state.peak_alive = max(state.peak_alive, alive)
  state.peak_bullets = max(state.peak_bullets, bullets)
end

function game.init()
  reset_state("title")
  music(-1)
  printh("radiant swarm init; ecs world=arena")
end

function game.start()
  if state.phase == "play" then return end
  reset_state("play")
  seed_stars(12)
  spawn_player()
  state.banner = "SIGNAL ACQUIRED"
  state.banner_color = 7
  state.banner_frames = 90
  music(0)
  sfx(5, 5)
end

function game.stress()
  game.start()
  state.debug_invulnerable = true
  state.frame = 360
  state.banner = "DEV: DENSE SWARM"
  state.banner_color = 47
  state.banner_frames = 120
  for index = 1, 8 do
    spawn_enemy(3, 18 + index * 18, 50 + (index % 3) * 20, true)
  end
end

function game.damage_boss(amount)
  amount = flr(amount or 0)
  world:each({"pos", "enemy"}, function(id, pos, enemy)
    if enemy.boss and not enemy.dead then
      enemy.hp = enemy.hp - amount
      state.boss_hp = max(0, enemy.hp)
      if enemy.hp <= 0 then kill_enemy(id, pos, enemy) end
    end
  end)
end

function game.update()
  if state.phase == "title" then
    state.frame = state.frame + 1
    update_motion()
    if btnp(4) then game.start() end
    return
  end
  if state.phase == "gameover" or state.phase == "victory" then
    state.frame = state.frame + 1
    update_motion()
    update_lifetimes_and_bounds()
    if btnp(4) and state.frame > 20 then game.start() end
    return
  end

  state.frame = state.frame + 1
  update_feedback()
  if state.banner_frames > 0 then state.banner_frames = state.banner_frames - 1 end
  move_and_fire_player()
  if btnp(5) then nova_bomb() end
  spawn_waves()
  update_enemies()
  update_motion()
  collide_player_shots()
  collide_hostile_bullets()
  update_pickups()
  update_lifetimes_and_bounds()
  update_peak()
end

function game.draw()
  render.draw(world, state)
end

function game.status()
  local player_pos, player = player_position()
  return {
    phase=state.phase,
    frame=state.frame,
    score=state.score,
    graze=state.graze,
    hp=state.hp,
    bombs=state.bombs,
    wave=state.wave,
    alive=world:count(),
    enemies=world:count({"enemy"}),
    enemy_bullets=world:count({"hostile"}),
    friendly_bullets=world:count({"friendly"}),
    particles=world:count({"particle"}),
    peak_alive=state.peak_alive,
    peak_bullets=state.peak_bullets,
    dropped_spawns=state.dropped_spawns,
    boss_hp=state.boss_hp,
    boss_phase=state.boss_phase,
    boss_transition=state.boss_transition,
    formation=state.formation,
    graze_chain=state.graze_chain,
    nova_flash=state.nova_flash,
    damage_flash=state.damage_flash,
    player=player_pos and {x=flr(player_pos.x),y=flr(player_pos.y),invulnerable=player.invulnerable} or nil,
  }
end

return game
