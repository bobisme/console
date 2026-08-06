-- Deterministic cart development hooks.
--
-- Carts receive only register(). Host list/invoke/lock closures are returned
-- separately and retained in the Lua registry, so replacing the public
-- global cannot alter the host control path.

local MAX_HOOKS = 32
local MAX_NAME_BYTES = 64
local MAX_DESCRIPTION_BYTES = 160
local hooks = {}
local order = {}
local locked = false

local function fail(message, level)
  error("devhook.register: " .. message, (level or 1) + 1)
end

local function valid_name(name)
  if type(name) ~= "string" then fail("name must be a string", 2) end
  if #name < 1 or #name > MAX_NAME_BYTES then
    fail("name must be 1-" .. MAX_NAME_BYTES .. " bytes", 2)
  end
  if not string.match(name, "^[%a_][%w_.-]*$") then
    fail("name must begin with a letter/_ and contain only letters, digits, _, ., or -", 2)
  end
end

local library = {}

function library.register(name, spec)
  if locked then fail("registration is closed after _init", 2) end
  valid_name(name)
  if hooks[name] ~= nil then fail("duplicate hook " .. name, 2) end
  if #order >= MAX_HOOKS then fail("cart may register at most " .. MAX_HOOKS .. " hooks", 2) end
  if type(spec) ~= "table" then fail("spec must be a table", 2) end
  for key, _ in pairs(spec) do
    if key ~= "description" and key ~= "phase" and key ~= "run" then
      fail("unknown spec field " .. tostring(key), 2)
    end
  end
  if type(spec.description) ~= "string" or #spec.description < 1 or #spec.description > MAX_DESCRIPTION_BYTES then
    fail("description must be a 1-" .. MAX_DESCRIPTION_BYTES .. " byte string", 2)
  end
  if spec.phase ~= "pre_frame" and spec.phase ~= "post_frame" then
    fail("phase must be pre_frame or post_frame", 2)
  end
  if type(spec.run) ~= "function" then fail("run must be a function", 2) end

  hooks[name] = {
    description = spec.description,
    phase = spec.phase,
    run = spec.run,
  }
  order[#order + 1] = name
end

local function list()
  local result = {}
  for index = 1, #order do
    local name = order[index]
    local hook = hooks[name]
    result[index] = {
      name = name,
      description = hook.description,
      phase = hook.phase,
    }
  end
  return result
end

local function invoke(name, phase, args)
  local hook = hooks[name]
  if hook == nil then error("unknown development hook " .. tostring(name), 2) end
  if hook.phase ~= phase then
    error("development hook " .. name .. " has phase " .. hook.phase .. ", not " .. tostring(phase), 2)
  end
  return hook.run(args)
end

local function lock()
  locked = true
end

return library, list, invoke, lock
