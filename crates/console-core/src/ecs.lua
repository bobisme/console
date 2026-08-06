-- Deterministic, deliberately small ECS for console carts.
--
-- Storage is private to these closures. Carts receive opaque world objects and
-- monotonically increasing integer entity handles; the host retains the
-- original inspector function separately in Lua's registry.

local DEFAULT_CAPACITY = 1024
local MAX_CAPACITY = 4096
local MAX_WORLDS = 16
local MAX_NAME_BYTES = 64
local MAX_COMPONENT_TYPES = 128
local MAX_SAFE_ID = 9007199254740991
local INSPECT_MAX_LIMIT = 128
local INSPECT_MAX_WITH = 16
local INSPECT_MAX_SELECT = 8
local INSPECT_MAX_FIELDS = 16
local INSPECT_CELL_BUDGET = 2048
local INSPECT_STRING_BUDGET = 32768
local INSPECT_STRING_MAX = 256

local states = setmetatable({}, { __mode = "k" })
local worlds = {}
local worlds_by_name = {}
local methods = {}

local function fail(who, message, level)
  error(who .. ": " .. message, (level or 1) + 1)
end

local function valid_name(value, who)
  if type(value) ~= "string" then
    fail(who, "name must be a string", 2)
  end
  if #value < 1 or #value > MAX_NAME_BYTES then
    fail(who, "name must be 1-" .. MAX_NAME_BYTES .. " bytes", 2)
  end
  if not string.match(value, "^[%a_][%w_.-]*$") then
    fail(who, "name must begin with a letter/_ and contain only letters, digits, _, ., or -", 2)
  end
  return value
end

local function integer(value, who, minimum, maximum)
  if type(value) ~= "number" or value ~= flr(value) or value < minimum or value > maximum then
    fail(who, "must be an integer in " .. minimum .. ".." .. maximum, 2)
  end
  return value
end

local function state(self, who)
  local value = states[self]
  if value == nil then
    fail(who, "expected an ecs world", 2)
  end
  return value
end

local function copy_components(value, who)
  if type(value) ~= "table" then
    fail(who, "components must be a table", 2)
  end
  local copy = {}
  local count = 0
  for name, component in pairs(value) do
    valid_name(name, who .. " component")
    if component == nil then
      fail(who, "component " .. name .. " cannot be nil", 2)
    end
    count = count + 1
    if count > MAX_COMPONENT_TYPES then
      fail(who, "entities may have at most " .. MAX_COMPONENT_TYPES .. " components", 2)
    end
    copy[name] = component
  end
  return copy
end

local function register_component_types(value, components, who)
  local missing = 0
  for name, _ in pairs(components) do
    if not value.component_names[name] then missing = missing + 1 end
  end
  if value.component_type_count + missing > MAX_COMPONENT_TYPES then
    fail(who, "world " .. value.name .. " reached component type limit " .. MAX_COMPONENT_TYPES, 2)
  end
  for name, _ in pairs(components) do
    if not value.component_names[name] then
      value.component_names[name] = true
      value.component_type_count = value.component_type_count + 1
    end
  end
end

local function names(value, who, maximum)
  if value == nil then return {} end
  if type(value) ~= "table" then
    fail(who, "must be an array of component names", 2)
  end
  local result = {}
  local count = 0
  for key, _ in pairs(value) do
    if type(key) ~= "number" or key ~= flr(key) or key < 1 then
      fail(who, "must be a dense array", 2)
    end
    count = count + 1
  end
  if count ~= #value or count > maximum then
    fail(who, "must be a dense array with at most " .. maximum .. " names", 2)
  end
  local seen = {}
  for index = 1, #value do
    local name = valid_name(value[index], who)
    if seen[name] then fail(who, "duplicate component " .. name, 2) end
    seen[name] = true
    result[index] = name
  end
  return result
end

local function matches(components, required)
  for index = 1, #required do
    if components[required[index]] == nil then return false end
  end
  return true
end

local function compact(value)
  if value.dead == 0 then return end
  local order = {}
  for index = 1, #value.order do
    local id = value.order[index]
    if value.entities[id] ~= nil then order[#order + 1] = id end
  end
  value.order = order
  value.dead = 0
end

local function apply(value, operation)
  local kind = operation[1]
  if kind == "spawn" then
    local id, components = operation[2], operation[3]
    value.entities[id] = components
    value.order[#value.order + 1] = id
    value.alive = value.alive + 1
    value.pending_spawns = value.pending_spawns - 1
  elseif kind == "despawn" then
    local id = operation[2]
    if value.entities[id] ~= nil then
      value.entities[id] = nil
      value.alive = value.alive - 1
      value.dead = value.dead + 1
    end
    value.queued_despawns[id] = nil
  elseif kind == "add" then
    local components = value.entities[operation[2]]
    if components ~= nil then components[operation[3]] = operation[4] end
  elseif kind == "remove" then
    local components = value.entities[operation[2]]
    if components ~= nil then components[operation[3]] = nil end
  end
end

local function enqueue(value, operation)
  if value.depth > 0 then
    value.pending[#value.pending + 1] = operation
  else
    apply(value, operation)
    if value.dead >= 64 and value.dead * 2 >= #value.order then compact(value) end
  end
end

local function flush(value)
  if value.depth ~= 0 or #value.pending == 0 then return end
  local pending = value.pending
  value.pending = {}
  for index = 1, #pending do apply(value, pending[index]) end
  compact(value)
end

function methods:name()
  return state(self, "world:name").name
end

function methods:spawn(components)
  local value = state(self, "world:spawn")
  if value.alive + value.pending_spawns >= value.capacity then
    fail("world:spawn", "world " .. value.name .. " reached capacity " .. value.capacity, 2)
  end
  if value.next_id > MAX_SAFE_ID then
    fail("world:spawn", "entity id space exhausted", 2)
  end
  local copied = copy_components(components, "world:spawn")
  register_component_types(value, copied, "world:spawn")
  local id = value.next_id
  value.next_id = id + 1
  value.pending_spawns = value.pending_spawns + 1
  enqueue(value, { "spawn", id, copied })
  return id
end

function methods:despawn(id)
  local value = state(self, "world:despawn")
  integer(id, "world:despawn entity", 1, MAX_SAFE_ID)
  if value.entities[id] == nil or value.queued_despawns[id] then return false end
  if value.depth > 0 then value.queued_despawns[id] = true end
  enqueue(value, { "despawn", id })
  return true
end

function methods:alive(id)
  local value = state(self, "world:alive")
  integer(id, "world:alive entity", 1, MAX_SAFE_ID)
  return value.entities[id] ~= nil
end

function methods:get(id, component)
  local value = state(self, "world:get")
  integer(id, "world:get entity", 1, MAX_SAFE_ID)
  component = valid_name(component, "world:get component")
  local components = value.entities[id]
  if components == nil then return nil end
  return components[component]
end

function methods:has(id, component)
  return self:get(id, component) ~= nil
end

function methods:add(id, component, component_value)
  local value = state(self, "world:add")
  integer(id, "world:add entity", 1, MAX_SAFE_ID)
  component = valid_name(component, "world:add component")
  if component_value == nil then fail("world:add", "component value cannot be nil", 2) end
  if value.entities[id] == nil or value.queued_despawns[id] then return false end
  register_component_types(value, { [component] = true }, "world:add")
  enqueue(value, { "add", id, component, component_value })
  return true
end

function methods:remove(id, component)
  local value = state(self, "world:remove")
  integer(id, "world:remove entity", 1, MAX_SAFE_ID)
  component = valid_name(component, "world:remove component")
  if value.entities[id] == nil or value.queued_despawns[id] then return false end
  enqueue(value, { "remove", id, component })
  return true
end

function methods:entities(required)
  local value = state(self, "world:entities")
  required = names(required, "world:entities", INSPECT_MAX_WITH)
  local result = {}
  for index = 1, #value.order do
    local id = value.order[index]
    local components = value.entities[id]
    if components ~= nil and matches(components, required) then result[#result + 1] = id end
  end
  return result
end

function methods:each(required, callback)
  local value = state(self, "world:each")
  required = names(required, "world:each", INSPECT_MAX_WITH)
  if type(callback) ~= "function" then fail("world:each", "callback must be a function", 2) end

  local selected = {}
  for index = 1, #value.order do
    local id = value.order[index]
    local components = value.entities[id]
    if components ~= nil and matches(components, required) then selected[#selected + 1] = id end
  end

  value.depth = value.depth + 1
  for index = 1, #selected do
    local id = selected[index]
    local components = value.entities[id]
    local arguments = {}
    for component_index = 1, #required do
      arguments[component_index] = components[required[component_index]]
    end
    local ok, message = pcall(callback, id, table.unpack(arguments, 1, #required))
    if not ok then
      value.depth = value.depth - 1
      if value.depth == 0 then flush(value) end
      error(message, 0)
    end
  end
  value.depth = value.depth - 1
  if value.depth == 0 then flush(value) end
  return #selected
end

function methods:count(required)
  local value = state(self, "world:count")
  if required == nil then return value.alive end
  required = names(required, "world:count", INSPECT_MAX_WITH)
  local count = 0
  for index = 1, #value.order do
    local components = value.entities[value.order[index]]
    if components ~= nil and matches(components, required) then count = count + 1 end
  end
  return count
end

function methods:clear()
  local value = state(self, "world:clear")
  if value.depth > 0 then fail("world:clear", "cannot clear during world:each", 2) end
  value.entities = {}
  value.order = {}
  value.alive = 0
  value.dead = 0
  value.pending = {}
  value.pending_spawns = 0
  value.queued_despawns = {}
end

function methods:stats()
  local value = state(self, "world:stats")
  local component_counts = {}
  for index = 1, #value.order do
    local components = value.entities[value.order[index]]
    if components ~= nil then
      for name, _ in pairs(components) do
        component_counts[name] = (component_counts[name] or 0) + 1
      end
    end
  end
  return {
    name = value.name,
    alive = value.alive,
    capacity = value.capacity,
    next_id = value.next_id,
    component_type_count = value.component_type_count,
    component_counts = component_counts,
  }
end

local world_metatable = {
  __index = methods,
  __metatable = "console ecs world",
  __tostring = function(self)
    local value = states[self]
    return value and ("ecs.world(" .. value.name .. ")") or "ecs.world(?)"
  end,
}

local ecs = {}

function ecs.world(name, options)
  name = valid_name(name, "ecs.world")
  if worlds_by_name[name] ~= nil then fail("ecs.world", "world " .. name .. " already exists", 2) end
  if #worlds >= MAX_WORLDS then fail("ecs.world", "world limit " .. MAX_WORLDS .. " reached", 2) end
  if options ~= nil and type(options) ~= "table" then fail("ecs.world", "options must be a table", 2) end
  local capacity = options and options.capacity or DEFAULT_CAPACITY
  capacity = integer(capacity, "ecs.world capacity", 1, MAX_CAPACITY)

  local world = setmetatable({}, world_metatable)
  states[world] = {
    name = name,
    capacity = capacity,
    entities = {},
    order = {},
    next_id = 1,
    alive = 0,
    dead = 0,
    depth = 0,
    pending = {},
    pending_spawns = 0,
    queued_despawns = {},
    component_names = {},
    component_type_count = 0,
  }
  worlds[#worlds + 1] = world
  worlds_by_name[name] = world
  return world
end

local function scalar(value, budget)
  if budget.cells <= 0 then
    budget.exhausted = true
    return nil, false
  end
  budget.cells = budget.cells - 1
  local kind = type(value)
  if kind == "nil" or kind == "boolean" then return value, true end
  if kind == "number" then
    if value ~= value or value == math.huge or value == -math.huge then return "<non-finite>", true end
    return value, true
  end
  if kind == "string" then
    local allowed = min(#value, INSPECT_STRING_MAX, budget.string_bytes)
    budget.string_bytes = budget.string_bytes - allowed
    if allowed < #value then budget.exhausted = true end
    return string.sub(value, 1, allowed), true
  end
  return "<" .. kind .. ">", true
end

local function selected_components(value)
  if value == nil then return {}, {} end
  if type(value) ~= "table" then fail("ecs.inspect select", "must be an object", 3) end
  local component_names = {}
  local fields = {}
  for component, selected_fields in pairs(value) do
    component = valid_name(component, "ecs.inspect select component")
    component_names[#component_names + 1] = component
    fields[component] = names(selected_fields, "ecs.inspect select fields", INSPECT_MAX_FIELDS)
  end
  if #component_names > INSPECT_MAX_SELECT then
    fail("ecs.inspect select", "may contain at most " .. INSPECT_MAX_SELECT .. " components", 3)
  end
  table.sort(component_names)
  return component_names, fields
end

local function inspect(world_name, options)
  world_name = valid_name(world_name, "ecs.inspect")
  if options == nil then options = {} end
  if type(options) ~= "table" then fail("ecs.inspect", "options must be a table", 2) end
  local world = worlds_by_name[world_name]
  if world == nil then fail("ecs.inspect", "no world named " .. world_name, 2) end
  local value = states[world]
  local required = names(options.with, "ecs.inspect with", INSPECT_MAX_WITH)
  local select_names, select_fields = selected_components(options.select)
  local limit = integer(options.limit or 64, "ecs.inspect limit", 1, INSPECT_MAX_LIMIT)
  local after = integer(options.after or 0, "ecs.inspect after", 0, MAX_SAFE_ID)
  local budget = {
    cells = INSPECT_CELL_BUDGET,
    string_bytes = INSPECT_STRING_BUDGET,
    exhausted = false,
  }

  local component_counts = {}
  local entities = {}
  local matched = 0
  local more_after = false
  for index = 1, #value.order do
    local id = value.order[index]
    local components = value.entities[id]
    if components ~= nil then
      for component, _ in pairs(components) do
        component_counts[component] = (component_counts[component] or 0) + 1
      end
      if matches(components, required) then
        matched = matched + 1
        if id > after then
          if #entities < limit then
            local selected = {}
            for select_index = 1, #select_names do
              if budget.cells <= 0 then budget.exhausted = true break end
              local component = select_names[select_index]
              local component_value = components[component]
              if component_value ~= nil then
                if type(component_value) == "table" then
                  local projected = {}
                  local fields = select_fields[component]
                  for field_index = 1, #fields do
                    if budget.cells <= 0 then budget.exhausted = true break end
                    local field = fields[field_index]
                    local projected_value, present = scalar(component_value[field], budget)
                    if present then projected[field] = projected_value end
                  end
                  selected[component] = projected
                else
                  local projected_value, present = scalar(component_value, budget)
                  if present then selected[component] = projected_value end
                end
              end
            end
            entities[#entities + 1] = { id = id, components = selected }
          else
            more_after = true
          end
        end
      end
    end
  end

  local next_after = after
  if #entities > 0 then next_after = entities[#entities].id end
  return {
    world = world_name,
    alive = value.alive,
    capacity = value.capacity,
    component_type_count = value.component_type_count,
    matched = matched,
    returned = #entities,
    truncated = more_after or budget.exhausted,
    budget_exhausted = budget.exhausted,
    next_after = next_after,
    component_counts = component_counts,
    entities = entities,
  }
end

ecs.inspect = inspect

return ecs, inspect
