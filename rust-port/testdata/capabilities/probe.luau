local helper = require("helper")

local Widget = {}

function Widget.render(self)
  return helper.help(self.name)
end

local function main()
  Widget.render(Widget)
end
