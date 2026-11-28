class Vector:
    def __add__(self, other):
        return self

    def __getitem__(self, key):
        return key

    def norm(self):
        return 1


class Resource:
    def __enter__(self):
        return self

    def __exit__(self, *args):
        return False

    def read(self):
        return 2


def make():
    return Vector()


def operators():
    a = make()
    b = make()
    total = a + b
    return total.norm()


def subscript():
    v = make()
    return v[0]


def comprehension():
    return [make().norm() for _ in [1, 2]]


def with_block():
    with Resource() as res:
        return res.read()


def branches(flag):
    if flag:
        v = make()
    else:
        v = Vector()
    return v.norm()


def loop():
    for v in [make(), make()]:
        v.norm()


def guarded():
    try:
        return make().norm()
    except ValueError:
        return None


class Holder:
    default = make()

    def use(self):
        return self.default.norm()


def matched(value):
    match value:
        case Vector() as v:
            return v.norm()
        case [first, *rest]:
            return make()
    return None


def lambdas():
    pick = lambda: make()
    return pick().norm()
