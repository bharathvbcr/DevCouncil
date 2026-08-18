require "json"
class Service
  def run
    JSON.generate({ok: true})
  end
end
