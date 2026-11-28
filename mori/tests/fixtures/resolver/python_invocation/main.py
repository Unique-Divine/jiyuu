import os.path


def helper():
    return 1


def other():
    return 2


class Box:
    def __init__(self):
        self.size = 0

    def grow(self):
        return self.size

    @staticmethod
    def build():
        return Box()

    @classmethod
    def create(cls):
        return cls()


HANDLERS = {"a": helper, "b": other}
square = lambda x: x * x


def direct():
    return helper()


def construct():
    box = Box()
    return box.grow()


def static_call():
    return Box.build()


def class_call():
    return Box.create()


def dispatch():
    return HANDLERS["a"]()


def lambda_call():
    return square(3)


def alias_call():
    fn = helper
    return fn()


def module_call():
    return os.path.join("a", "b")


def nested():
    def inner():
        return other()

    return inner()
