local helper = require("helper")

local Widget = {}
Widget.__index = Widget

function Widget.render(self)
  return helper.help(self.name)
end

function main()
  local w = setmetatable({}, Widget)
  Widget.render(w)
end
