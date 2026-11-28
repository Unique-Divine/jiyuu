class Root:
    def __init__(self):
        self.ready = True

    def work(self):
        return 0


class Left(Root):
    pass


class Right(Root):
    def work(self):
        return 1


class Child(Left, Right):
    def __init__(self):
        super().__init__()

    def direct(self):
        return self.work()

    def via_super(self):
        return super().work()


class First:
    def work(self):
        return 2


class Second(First):
    pass


class Third(First):
    def work(self):
        return 3


class Bad(Second, Third):
    def probe(self):
        return super().work()


class P:
    def run(self):
        return 4


class Q:
    def run(self):
        return 5


class PQ(P, Q):
    pass


class QP(Q, P):
    pass


class Broken(PQ, QP):
    def call(self):
        return self.run()

    def up(self):
        return super().run()
