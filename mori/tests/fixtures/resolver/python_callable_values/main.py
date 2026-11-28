import functools


def helper(value):
    return value


def other(value):
    return value


class Box:
    def method(self, value):
        return value


def apply(fn, value):
    return fn(value)


def passes_function():
    return apply(helper, 1)


def passes_method():
    box = Box()
    return apply(box.method, 2)


def partial():
    return functools.partial(helper, 3)


def keyword():
    return apply(fn=other, value=4)


def stored():
    fn = helper
    return apply(fn, 5)


def conditional(flag):
    return apply(helper if flag else other, 6)
