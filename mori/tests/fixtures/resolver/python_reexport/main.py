from python_reexport import pkg
from python_reexport.pkg import Widget, helper


def direct():
    return helper()


def via_package():
    return pkg.helper()


def make():
    return Widget()


def starred_name():
    return pkg.starred()


def chained():
    return pkg.deep()


def hidden():
    return pkg._hidden()
