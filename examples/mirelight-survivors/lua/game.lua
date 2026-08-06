local spatial = require("spatial")
local render = require("render")

local game = {}
local world = ecs.world("survivors", {capacity=1100})

local ARENA_W, ARENA_H = 384, 512
local SOFT_CAP = 997
local STRESS_ENEMIES = 820
local PROJECTILE_CAP = 96
local PICKUP_CAP = 32
local PARTICLE_CAP = 48
local grid = spatial.new(ARENA_W, ARENA_H, 16)
local candidates = {}

local state = {}
local player_id = nil
local enemy_serial = 0
local projectile_serial = 0
local effect_count = 0
local effect_x = {}
local effect_y = {}
local effect_color = {}

local function clamp(value, low, high)
  if value < low then return low end
  if value > high then return high end
  return value
end

local function fresh_state()
  state = {
    schema=1,
    phase="title",
    mode="normal",
    dense_selected=false,
    frame=0,
    score=0,
    hp=6,
    level=1,
    xp=0,
    next_xp=18,
    pending_upgrades=0,
    level_selection=1,
    damage=2,
    fire_delay=7,
    magnet=54,
    dash_cooldown=0,
    dash_flash=0,
    pulse_cooldown=0,
    pulse_flash=0,
    damage_flash=0,
    banner="",
    banner_frames=0,
    finale=false,
    invulnerable=false,
    alive=0,
    peak_alive=0,
    stress_min_alive=0,
    enemies=0,
    projectiles=0,
    pickups=0,
    particles=0,
    spawned_total=0,
    despawned_total=0,
    dropped_spawns=0,
    effect_budget_skips=0,
    kills=0,
    collision_candidates_last=0,
    collision_tests_last=0,
    collision_hits_last=0,
    collision_candidates_total=0,
    collision_tests_total=0,
    collision_hits_total=0,
    collision_naive_pairs_total=0,
    grid_active_cells=0,
    grid_max_bucket=0,
    population_target=0,
  }
end

local function reset_world()
  world:clear()
  fresh_state()
  player_id = nil
  enemy_serial = 0
  projectile_serial = 0
  effect_count = 0
  spatial.begin_frame(grid)
  spatial.finish_frame(grid)
end

local function safe_spawn(components, category)
  if state.alive >= SOFT_CAP then
    state.dropped_spawns = state.dropped_spawns + 1
    return nil
  end
  local id = world:spawn(components)
  state.alive = state.alive + 1
  state.spawned_total = state.spawned_total + 1
  if category then state[category] = state[category] + 1 end
  if state.alive > state.peak_alive then state.peak_alive = state.alive end
  return id
end

local function remove_entity(id, category)
  if not world:despawn(id) then return false end
  state.alive = state.alive - 1
  state.despawned_total = state.despawned_total + 1
  if category then state[category] = state[category] - 1 end
  return true
end

local function spawn_player()
  player_id = safe_spawn({
    pos={x=ARENA_W / 2, y=ARENA_H / 2},
    player={radius=4, invulnerability=0},
  })
end

local function enemy_spec(kind, elite)
  if kind == 2 then return 5, 0.48, 4, 2 end
  if kind == 3 then return 10, 0.24, 5, 2 end
  if kind == 4 then return 22, 0.14, 6, 3 end
  local hp = elite and 18 or 7
  return hp, elite and 0.34 or 0.29, elite and 6 or 4, elite and 3 or 1
end

local function spawn_enemy_at(x, y, forced_kind, forced_elite)
  enemy_serial = enemy_serial + 1
  local kind = forced_kind or (1 + enemy_serial % 4)
  local elite = forced_elite or (enemy_serial % 97 == 0)
  local hp, speed, radius, damage = enemy_spec(kind, elite)
  return safe_spawn({
    pos={x=clamp(x, 4, ARENA_W - 5), y=clamp(y, 4, ARENA_H - 5)},
    enemy={
      kind=kind,
      hp=hp,
      max_hp=hp,
      speed=speed,
      radius=radius,
      damage=damage,
      elite=elite,
      dead=false,
      hit_flash=0,
      age=enemy_serial % 120,
    },
  }, "enemies")
end

local function spawn_enemy_near_player()
  local player_pos = world:get(player_id, "pos")
  local edge = enemy_serial % 4
  local x, y
  if edge == 0 then
    x = player_pos.x - 116 - rnd(72)
    y = player_pos.y - 152 + rnd(304)
  elseif edge == 1 then
    x = player_pos.x + 116 + rnd(72)
    y = player_pos.y - 152 + rnd(304)
  elseif edge == 2 then
    x = player_pos.x - 152 + rnd(304)
    y = player_pos.y - 184 - rnd(72)
  else
    x = player_pos.x - 152 + rnd(304)
    y = player_pos.y + 184 + rnd(72)
  end
  spawn_enemy_at(x, y)
end

local function spawn_projectile(x, y, vx, vy, damage, pierce, life)
  if state.projectiles >= PROJECTILE_CAP then
    state.effect_budget_skips = state.effect_budget_skips + 1
    return nil
  end
  projectile_serial = projectile_serial + 1
  return safe_spawn({
    pos={x=x, y=y},
    projectile={
      kind=projectile_serial % 5 == 0 and "thorn" or "seed",
      vx=vx,
      vy=vy,
      damage=damage or state.damage,
      radius=2,
      pierce=pierce or 1,
      ttl=life or 105,
    },
  }, "projectiles")
end

local function spawn_pickup(x, y, value)
  if state.pickups >= PICKUP_CAP or state.alive >= SOFT_CAP then
    state.effect_budget_skips = state.effect_budget_skips + 1
    return nil
  end
  return safe_spawn({
    pos={x=x, y=y},
    pickup={kind="dew", value=value or 2, age=0},
  }, "pickups")
end

local function spawn_particle(x, y, color)
  if state.particles >= PARTICLE_CAP or state.alive >= SOFT_CAP then
    state.effect_budget_skips = state.effect_budget_skips + 1
    return nil
  end
  local turn = rnd(1)
  local speed = 0.25 + rnd(0.65)
  return safe_spawn({
    pos={x=x, y=y},
    particle={
      kind="spore",
      vx=cos(turn) * speed,
      vy=sin(turn) * speed,
      ttl=18 + flr(rnd(30)),
      color=color or 29,
    },
  }, "particles")
end

local function queue_effect(x, y, color)
  effect_count = effect_count + 1
  effect_x[effect_count] = x
  effect_y[effect_count] = y
  effect_color[effect_count] = color
end

local function flush_effects()
  for index = 1, effect_count do
    if state.particles < PARTICLE_CAP and state.alive < SOFT_CAP then
      spawn_particle(effect_x[index], effect_y[index], effect_color[index])
    end
    if index % 4 == 0 and state.pickups < PICKUP_CAP and state.alive < SOFT_CAP then
      spawn_pickup(effect_x[index], effect_y[index], 2)
    end
  end
  effect_count = 0
end

local function award_xp(amount)
  amount = flr(amount or 0)
  if amount <= 0 or state.phase == "title" then return end
  state.xp = state.xp + amount
  while state.xp >= state.next_xp do
    state.xp = state.xp - state.next_xp
    state.level = state.level + 1
    state.next_xp = 14 + state.level * 8
    if state.mode == "stress" then
      local choice = 1 + (state.level % 3)
      if choice == 1 then state.damage = state.damage + 1 end
      if choice == 2 then state.fire_delay = max(3, state.fire_delay - 1) end
      if choice == 3 then state.magnet = min(100, state.magnet + 8) end
    else state.pending_upgrades = state.pending_upgrades + 1 end
  end
  if state.mode ~= "stress" and state.pending_upgrades > 0 then
    state.phase = "levelup"
    state.level_selection = 1
  end
end

local function kill_enemy(id, pos, enemy)
  if enemy.dead then return end
  enemy.dead = true
  if remove_entity(id, "enemies") then
    state.kills = state.kills + 1
    state.score = state.score + (enemy.elite and 75 or 10 + enemy.kind * 3)
    queue_effect(pos.x, pos.y, enemy.elite and 31 or 29)
  end
end

local function nearest_enemy(x, y, radius)
  local count = spatial.query(grid, x, y, radius, candidates)
  local best_id = nil
  local best_distance = radius * radius + 1
  for index = 1, count do
    local id = candidates[index]
    local enemy = world:get(id, "enemy")
    local pos = world:get(id, "pos")
    if enemy and pos and not enemy.dead then
      local dx = pos.x - x
      local dy = pos.y - y
      local distance = dx * dx + dy * dy
      if distance < best_distance or (distance == best_distance and (best_id == nil or id < best_id)) then
        best_id = id
        best_distance = distance
      end
    end
  end
  return best_id, best_distance
end

local function fire_at_nearest()
  if state.projectiles >= PROJECTILE_CAP then return end
  local player_pos = world:get(player_id, "pos")
  local target_id = nearest_enemy(player_pos.x, player_pos.y, 150)
  local dx, dy = 0, -1
  if target_id then
    local target = world:get(target_id, "pos")
    dx = target.x - player_pos.x
    dy = target.y - player_pos.y
    local length = math.sqrt(dx * dx + dy * dy)
    if length > 0.01 then dx, dy = dx / length, dy / length end
  end
  spawn_projectile(player_pos.x, player_pos.y, dx * 3.7, dy * 3.7, state.damage, state.level >= 5 and 2 or 1)
end

local function move_player()
  local pos = world:get(player_id, "pos")
  local player = world:get(player_id, "player")
  local dx = (btn(1) and 1 or 0) - (btn(0) and 1 or 0)
  local dy = (btn(3) and 1 or 0) - (btn(2) and 1 or 0)
  local speed = 1.45
  if dx ~= 0 and dy ~= 0 then speed = 1.03 end
  if btnp(4) and state.dash_cooldown == 0 and (dx ~= 0 or dy ~= 0) then
    speed = speed * 8
    state.dash_cooldown = 45
    state.dash_flash = 12
    player.invulnerability = max(player.invulnerability, 12)
    sfx(1, 5)
  end
  pos.x = clamp(pos.x + dx * speed, 6, ARENA_W - 7)
  pos.y = clamp(pos.y + dy * speed, 8, ARENA_H - 9)
  if player.invulnerability > 0 then player.invulnerability = player.invulnerability - 1 end
  if state.dash_cooldown > 0 then state.dash_cooldown = state.dash_cooldown - 1 end
  if state.pulse_cooldown > 0 then state.pulse_cooldown = state.pulse_cooldown - 1 end
end

local function damage_player(amount)
  amount = flr(amount or 0)
  if amount <= 0 or state.phase ~= "play" then return end
  local player = world:get(player_id, "player")
  if state.invulnerable or player.invulnerability > 0 then return end
  state.hp = max(0, state.hp - amount)
  player.invulnerability = 50
  state.damage_flash = 18
  sfx(3, 5)
  if state.hp <= 0 then
    state.phase = "gameover"
    state.banner = "THE GARDEN CLOSES"
    state.banner_frames = 9999
    music(2)
  end
end

local function pulse()
  if state.pulse_cooldown > 0 then return end
  state.pulse_cooldown = 180
  state.pulse_flash = 24
  local pos = world:get(player_id, "pos")
  local count = spatial.query(grid, pos.x, pos.y, 58, candidates)
  for index = 1, count do
    local id = candidates[index]
    local enemy = world:get(id, "enemy")
    local enemy_pos = world:get(id, "pos")
    if enemy and enemy_pos and not enemy.dead then
      local dx = enemy_pos.x - pos.x
      local dy = enemy_pos.y - pos.y
      if dx * dx + dy * dy <= 58 * 58 then
        enemy.hp = enemy.hp - (state.damage + 4)
        enemy.hit_flash = 7
        if enemy.hp <= 0 then kill_enemy(id, enemy_pos, enemy) end
      end
    end
  end
  sfx(2, 4)
end

local function update_entities()
  local player_pos = world:get(player_id, "pos")
  spatial.begin_frame(grid)
  world:each({"pos"}, function(id, pos)
    local enemy = world:get(id, "enemy")
    if enemy then
      enemy.age = enemy.age + 1
      if enemy.hit_flash > 0 then enemy.hit_flash = enemy.hit_flash - 1 end
      local dx = player_pos.x - pos.x
      local dy = player_pos.y - pos.y
      local length = math.sqrt(dx * dx + dy * dy)
      if length > 0.01 then dx, dy = dx / length, dy / length end
      local pressure_x, pressure_y = spatial.pressure(grid, pos.x, pos.y)
      local weave = ((id * 17 + state.frame) % 31 - 15) * 0.002
      pos.x = clamp(pos.x + dx * enemy.speed + pressure_x - dy * weave, 3, ARENA_W - 4)
      pos.y = clamp(pos.y + dy * enemy.speed + pressure_y + dx * weave, 3, ARENA_H - 4)
      spatial.insert(grid, id, pos.x, pos.y)
    else
      local projectile = world:get(id, "projectile")
      if projectile then
        pos.x = pos.x + projectile.vx
        pos.y = pos.y + projectile.vy
        projectile.ttl = projectile.ttl - 1
        if projectile.ttl <= 0 or pos.x < -8 or pos.x > ARENA_W + 8 or pos.y < -8 or pos.y > ARENA_H + 8 then
          remove_entity(id, "projectiles")
        end
      else
        local pickup = world:get(id, "pickup")
        if pickup then
          pickup.age = pickup.age + 1
          local dx = player_pos.x - pos.x
          local dy = player_pos.y - pos.y
          local distance = dx * dx + dy * dy
          if distance < state.magnet * state.magnet and distance > 0.01 then
            local length = math.sqrt(distance)
            local speed = distance < 24 * 24 and 2.8 or 1.25
            pos.x = pos.x + dx / length * speed
            pos.y = pos.y + dy / length * speed
          end
          if distance < 7 * 7 and remove_entity(id, "pickups") then
            award_xp(pickup.value)
            state.score = state.score + 5
            sfx(0, 5)
          end
        else
          local particle = world:get(id, "particle")
          if particle then
            pos.x = pos.x + particle.vx
            pos.y = pos.y + particle.vy
            particle.vx = particle.vx * 0.94
            particle.vy = particle.vy * 0.94
            particle.ttl = particle.ttl - 1
            if particle.ttl <= 0 then remove_entity(id, "particles") end
          end
        end
      end
    end
  end)
  spatial.finish_frame(grid)
  state.grid_active_cells = grid.active_cells
  state.grid_max_bucket = max(state.grid_max_bucket, grid.max_bucket)
end

local function collide_projectiles()
  state.collision_naive_pairs_total = state.collision_naive_pairs_total + state.enemies * state.projectiles
  world:each({"pos", "projectile"}, function(id, pos, projectile)
    local count = spatial.query(grid, pos.x, pos.y, projectile.radius + 7, candidates)
    state.collision_candidates_last = state.collision_candidates_last + count
    local projectile_alive = true
    for index = 1, count do
      if projectile_alive then
        local enemy_id = candidates[index]
        local enemy = world:get(enemy_id, "enemy")
        local enemy_pos = world:get(enemy_id, "pos")
        if enemy and enemy_pos and not enemy.dead then
          state.collision_tests_last = state.collision_tests_last + 1
          local dx = enemy_pos.x - pos.x
          local dy = enemy_pos.y - pos.y
          local radius = enemy.radius + projectile.radius
          if dx * dx + dy * dy <= radius * radius then
            state.collision_hits_last = state.collision_hits_last + 1
            enemy.hp = enemy.hp - projectile.damage
            enemy.hit_flash = 4
            projectile.pierce = projectile.pierce - 1
            if enemy.hp <= 0 then kill_enemy(enemy_id, enemy_pos, enemy) end
            if projectile.pierce <= 0 then
              projectile_alive = false
              remove_entity(id, "projectiles")
            end
          end
        end
      end
    end
  end)
  state.collision_candidates_total = state.collision_candidates_total + state.collision_candidates_last
  state.collision_tests_total = state.collision_tests_total + state.collision_tests_last
  state.collision_hits_total = state.collision_hits_total + state.collision_hits_last
end

local function collide_player()
  if state.invulnerable then return end
  local pos = world:get(player_id, "pos")
  local count = spatial.query(grid, pos.x, pos.y, 12, candidates)
  for index = 1, count do
    local enemy = world:get(candidates[index], "enemy")
    local enemy_pos = world:get(candidates[index], "pos")
    if enemy and enemy_pos and not enemy.dead then
      local dx = enemy_pos.x - pos.x
      local dy = enemy_pos.y - pos.y
      local radius = enemy.radius + 4
      if dx * dx + dy * dy <= radius * radius then
        damage_player(enemy.damage)
        return
      end
    end
  end
end

local function target_enemies()
  if state.mode == "stress" then return STRESS_ENEMIES end
  local target = min(STRESS_ENEMIES, 40 + flr(state.frame / 4))
  if state.finale then target = STRESS_ENEMIES end
  return target
end

local function replenish_enemies()
  local target = target_enemies()
  state.population_target = target
  local budget = state.mode == "stress" and 64 or 5
  while state.enemies < target and budget > 0 do
    spawn_enemy_near_player()
    budget = budget - 1
  end
end

local function replenish_stress_transients()
  if state.mode ~= "stress" then return end
  local player_pos = world:get(player_id, "pos")
  local projectile_budget = 16
  while state.projectiles < 64 and projectile_budget > 0 do
    local turn = (projectile_serial % 64) / 64
    spawn_projectile(
      player_pos.x + cos(turn) * 22,
      player_pos.y + sin(turn) * 22,
      cos(turn) * 2.7,
      sin(turn) * 2.7,
      state.damage,
      1,
      105
    )
    projectile_budget = projectile_budget - 1
  end
  while state.pickups < 16 and state.alive < SOFT_CAP do
    local index = state.spawned_total
    spawn_pickup(8 + ((index * 43) % (ARENA_W - 16)), 8 + ((index * 71) % (ARENA_H - 16)), 2)
  end
  while state.particles < 16 and state.alive < SOFT_CAP do
    local index = state.spawned_total
    spawn_particle(8 + ((index * 53) % (ARENA_W - 16)), 8 + ((index * 29) % (ARENA_H - 16)), 29 + index % 3)
  end
end

local function seed_stress_population()
  for index = 1, STRESS_ENEMIES do
    local x = 6 + ((index * 37) % (ARENA_W - 12))
    local y = 8 + ((index * 61) % (ARENA_H - 16))
    spawn_enemy_at(x, y, 1 + index % 4, index % 97 == 0)
  end
  local player_pos = world:get(player_id, "pos")
  for index = 1, 64 do
    local turn = index / 64
    spawn_projectile(
      player_pos.x + cos(turn) * 26,
      player_pos.y + sin(turn) * 26,
      cos(turn) * 2.4,
      sin(turn) * 2.4,
      2,
      1,
      90 + index % 30
    )
  end
  for index = 1, PICKUP_CAP do
    spawn_pickup(8 + ((index * 43) % (ARENA_W - 16)), 8 + ((index * 71) % (ARENA_H - 16)), 2)
  end
  for index = 1, PARTICLE_CAP do
    spawn_particle(8 + ((index * 53) % (ARENA_W - 16)), 8 + ((index * 29) % (ARENA_H - 16)), 29 + index % 3)
  end
end

local function choose_upgrade()
  if state.level_selection == 1 then
    state.damage = state.damage + 1
    state.banner = "THORNS CUT DEEPER"
  elseif state.level_selection == 2 then
    state.fire_delay = max(3, state.fire_delay - 1)
    state.banner = "SEEDS FLY FASTER"
  else
    state.magnet = min(100, state.magnet + 12)
    state.hp = min(8, state.hp + 1)
    state.banner = "DEW CALLS HOME"
  end
  state.banner_frames = 90
  state.pending_upgrades = max(0, state.pending_upgrades - 1)
  state.phase = state.pending_upgrades > 0 and "levelup" or "play"
  sfx(4, 5)
end

local function update_levelup()
  if btnp(2) then state.level_selection = state.level_selection == 1 and 3 or state.level_selection - 1 end
  if btnp(3) then state.level_selection = state.level_selection == 3 and 1 or state.level_selection + 1 end
  if btnp(4) then choose_upgrade() end
end

function game.init()
  reset_world()
  music(-1)
  printh("mirelight survivors init; ecs world=survivors; stress floor=800")
end

function game.start(stress_mode)
  reset_world()
  state.phase = "play"
  state.mode = stress_mode and "stress" or "normal"
  state.invulnerable = stress_mode and true or false
  state.banner = stress_mode and "DENSITY GARDEN: 800+" or "SURVIVE UNTIL DAWN"
  state.banner_frames = 150
  spawn_player()
  if stress_mode then
    seed_stress_population()
    state.stress_min_alive = state.alive
  else
    for _ = 1, 40 do spawn_enemy_near_player() end
  end
  music(stress_mode and 1 or 0)
  sfx(4, 5)
end

function game.grant_xp(amount)
  award_xp(amount)
end

function game.damage_player(amount)
  damage_player(amount)
end

function game.enter_finale()
  if state.phase == "title" then return end
  state.finale = true
  state.frame = max(state.frame, 3600)
  state.banner = "THE ROOT-MOON RISES"
  state.banner_frames = 180
  music(1)
end

function game.update()
  state.frame = state.frame + 1
  if state.phase == "title" then
    if btnp(5) then state.dense_selected = not state.dense_selected end
    if btnp(4) then game.start(state.dense_selected) end
    return
  end
  if state.phase == "levelup" then
    update_levelup()
    return
  end
  if state.phase == "gameover" or state.phase == "victory" then
    if btnp(4) then game.start(false) end
    if btnp(5) then game.start(true) end
    return
  end

  state.collision_candidates_last = 0
  state.collision_tests_last = 0
  state.collision_hits_last = 0
  if state.banner_frames > 0 then state.banner_frames = state.banner_frames - 1 end
  if state.dash_flash > 0 then state.dash_flash = state.dash_flash - 1 end
  if state.pulse_flash > 0 then state.pulse_flash = state.pulse_flash - 1 end
  if state.damage_flash > 0 then state.damage_flash = state.damage_flash - 1 end

  move_player()
  if btnp(5) then pulse() end
  if state.frame % state.fire_delay == 0 then fire_at_nearest() end
  update_entities()
  collide_projectiles()
  collide_player()
  replenish_enemies()
  replenish_stress_transients()
  flush_effects()

  if state.mode == "stress" then
    if state.stress_min_alive == 0 then state.stress_min_alive = state.alive end
    state.stress_min_alive = min(state.stress_min_alive, state.alive)
  end
  if not state.finale and state.frame >= 3600 then game.enter_finale() end
  if state.frame >= 4500 then
    state.phase = "victory"
    state.banner = "DAWN THROUGH A THOUSAND LEAVES"
    state.banner_frames = 9999
    music(2)
  end
end

function game.status()
  return {
    schema=state.schema,
    phase=state.phase,
    mode=state.mode,
    dense_selected=state.dense_selected,
    frame=state.frame,
    score=state.score,
    hp=state.hp,
    level=state.level,
    xp=state.xp,
    pending_upgrades=state.pending_upgrades,
    alive=world:count(),
    peak_alive=state.peak_alive,
    stress_min_alive=state.stress_min_alive,
    enemies=state.enemies,
    projectiles=state.projectiles,
    pickups=state.pickups,
    particles=state.particles,
    spawned_total=state.spawned_total,
    despawned_total=state.despawned_total,
    dropped_spawns=state.dropped_spawns,
    effect_budget_skips=state.effect_budget_skips,
    collision_candidates_last=state.collision_candidates_last,
    collision_tests_last=state.collision_tests_last,
    collision_hits_last=state.collision_hits_last,
    collision_candidates_total=state.collision_candidates_total,
    collision_tests_total=state.collision_tests_total,
    collision_hits_total=state.collision_hits_total,
    collision_naive_pairs_total=state.collision_naive_pairs_total,
    grid_active_cells=state.grid_active_cells,
    grid_max_bucket=state.grid_max_bucket,
    population_target=state.population_target,
    invulnerable=state.invulnerable,
    within_entity_ceiling=state.peak_alive <= 1000,
    spatial_reduction=state.collision_naive_pairs_total == 0 or state.collision_candidates_total * 10 < state.collision_naive_pairs_total,
    all_categories_live=state.enemies > 0 and state.projectiles > 0 and state.pickups > 0 and state.particles > 0,
  }
end

function game.draw()
  render.draw(world, state, player_id, ARENA_W, ARENA_H)
end

return game
