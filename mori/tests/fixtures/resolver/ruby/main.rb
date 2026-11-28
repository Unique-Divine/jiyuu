class Admin::User
  def initialize
  end
  def name
    'n'
  end
  def shout
    self.name
  end
end
class Outside
  def probe
    u = Admin::User.new
    u.name
  end
end
