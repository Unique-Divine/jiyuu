class Foo
  def bar
    1
  end
end
class Worker
  def run
    a = Foo.new
    [1].each { |a| a.bar }
    [1].each do |x, (y, z), *rest; loc|
      x.bar
      y.bar
      rest.bar
      loc.bar
    end
    f = ->(a) { a.bar }
    a.bar
  end
end
