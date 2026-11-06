import os
import os.path as osp
from collections import OrderedDict, defaultdict as dd

LIMIT = 10
counter = 0


def total(values, start=0, *rest, scale: int = 1, **options):
    result = start
    for value in values:
        result += value * scale
    squares = [v * v for v in values if v < LIMIT]
    if (n := len(squares)) > LIMIT:
        result = n
    return result + len(rest) + len(options)


def bump():
    global counter
    counter = counter + 1
    return os.getcwd()


def outer():
    count = 0

    def inner():
        nonlocal count
        count += 1
        return count

    return inner


class Invoice:
    rate = 2

    def __init__(self, lines):
        self.lines = lines
        self.rate = rate

    def amount(self):
        return self.total() * self.rate

    def total(self):
        return total(self.lines, scale=Invoice.rate)

    shown = rate * 2


def handle(path):
    try:
        with open(path) as handle_file:
            return handle_file.read()
    except OSError as error:
        return str(error)
    later()


def later():
    return OrderedDict(), dd(list), osp.sep
