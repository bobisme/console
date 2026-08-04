local hud = {}

function hud.draw(status)
  rectfill(8, 8, 183, 39, 0)
  rect(8, 8, 183, 39, 15)
  print("SOURCE HOPPER", 96, 14, 63, "center")
  print("MOVE L/R  JUMP A", 96, 25, 23, "center")
  print("MOTHS "..status.score, 184, 46, 31, "right")
end

return hud
