from typing import Dict, List, Optional, Union


class Engine:
    def start(self):
        return 1


class Wheel:
    def spin(self):
        return 2


def plain(engine: Engine):
    return engine.start()


def optional(engine: Optional[Engine]):
    return engine.start()


def union(part: Union[Engine, Wheel]):
    return part.spin()


def quoted(engine: "Engine"):
    return engine.start()


def listed(wheels: List[Wheel]):
    for wheel in wheels:
        wheel.spin()


def mapped(parts: Dict[str, Wheel]):
    for wheel in parts.values():
        wheel.spin()


def pep604(part: Engine | None):
    return part.start()


def make() -> Engine:
    return Engine()


def chained():
    return make().start()


def local():
    engine: Engine = make()
    return engine.start()
