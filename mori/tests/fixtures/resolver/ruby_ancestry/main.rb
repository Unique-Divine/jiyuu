module Greetable
  def greet
    'hello'
  end
end

class Base
  def helper
    1
  end
end

class Child < Base
  include Greetable

  def probe
    self.helper
    self.greet
  end
end
