def double(n):
    return n + n


def add(a, b):
    return double(a) + b


class Calc:
    def run(self):
        return add(1, 2)
