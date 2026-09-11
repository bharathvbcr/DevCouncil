require 'example/helper'

class Widget < BaseWidget
  include Renderable

  def render
    Helper.help(@name)
  end
end

def main
  w = Widget.new
  w.render
end
