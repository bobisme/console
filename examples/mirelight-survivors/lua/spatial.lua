local spatial = {}

local function clamp(value, low, high)
  if value < low then return low end
  if value > high then return high end
  return value
end

local function make_buffer(columns, rows)
  local result = {}
  for index = 1, columns * rows do
    result[index] = {count=0, sum_x=0, sum_y=0, ids={}}
  end
  return result
end

function spatial.new(width, height, cell_size)
  local columns = flr((width + cell_size - 1) / cell_size)
  local rows = flr((height + cell_size - 1) / cell_size)
  return {
    width=width,
    height=height,
    cell_size=cell_size,
    columns=columns,
    rows=rows,
    buffers={make_buffer(columns, rows), make_buffer(columns, rows)},
    read_index=1,
    write_index=2,
    active_cells=0,
    max_bucket=0,
  }
end

local function cell_coordinate(value, cell_size, maximum)
  return clamp(flr(value / cell_size), 0, maximum - 1)
end

local function bucket_index(grid, column, row)
  return row * grid.columns + column + 1
end

function spatial.begin_frame(grid)
  grid.write_index = grid.read_index == 1 and 2 or 1
  local buffer = grid.buffers[grid.write_index]
  for index = 1, #buffer do
    local bucket = buffer[index]
    bucket.count = 0
    bucket.sum_x = 0
    bucket.sum_y = 0
  end
  grid.active_cells = 0
  grid.max_bucket = 0
end

function spatial.insert(grid, id, x, y)
  local column = cell_coordinate(x, grid.cell_size, grid.columns)
  local row = cell_coordinate(y, grid.cell_size, grid.rows)
  local bucket = grid.buffers[grid.write_index][bucket_index(grid, column, row)]
  if bucket.count == 0 then grid.active_cells = grid.active_cells + 1 end
  bucket.count = bucket.count + 1
  bucket.ids[bucket.count] = id
  bucket.sum_x = bucket.sum_x + x
  bucket.sum_y = bucket.sum_y + y
  if bucket.count > grid.max_bucket then grid.max_bucket = bucket.count end
end

function spatial.finish_frame(grid)
  grid.read_index = grid.write_index
end

function spatial.query(grid, x, y, radius, output)
  local first_column = cell_coordinate(x - radius, grid.cell_size, grid.columns)
  local last_column = cell_coordinate(x + radius, grid.cell_size, grid.columns)
  local first_row = cell_coordinate(y - radius, grid.cell_size, grid.rows)
  local last_row = cell_coordinate(y + radius, grid.cell_size, grid.rows)
  local buffer = grid.buffers[grid.read_index]
  local count = 0
  for row = first_row, last_row do
    for column = first_column, last_column do
      local bucket = buffer[bucket_index(grid, column, row)]
      for item = 1, bucket.count do
        count = count + 1
        output[count] = bucket.ids[item]
      end
    end
  end
  return count
end

function spatial.pressure(grid, x, y)
  local column = cell_coordinate(x, grid.cell_size, grid.columns)
  local row = cell_coordinate(y, grid.cell_size, grid.rows)
  local buffer = grid.buffers[grid.read_index]
  local count = 0
  local sum_x = 0
  local sum_y = 0
  local first_column = max(0, column - 1)
  local last_column = min(grid.columns - 1, column + 1)
  local first_row = max(0, row - 1)
  local last_row = min(grid.rows - 1, row + 1)
  for near_row = first_row, last_row do
    for near_column = first_column, last_column do
      local bucket = buffer[bucket_index(grid, near_column, near_row)]
      count = count + bucket.count
      sum_x = sum_x + bucket.sum_x
      sum_y = sum_y + bucket.sum_y
    end
  end
  if count <= 1 then return 0, 0, count end
  local center_x = sum_x / count
  local center_y = sum_y / count
  local dx = x - center_x
  local dy = y - center_y
  local length = math.sqrt(dx * dx + dy * dy)
  if length < 0.01 then return 0, 0, count end
  local strength = min(0.34, (count - 1) * 0.018)
  return dx / length * strength, dy / length * strength, count
end

return spatial
