STORE = {}


def identity(value):
    return value


def audit(user):
    print(user.id)
    return True


def first(a, b):
    return a


def by_name(a, b=None):
    return b


def collect(*args):
    return args


def options(**kwargs):
    return kwargs


def save(item):
    STORE["last"] = item


class Repo:
    def __init__(self, conn):
        self.conn = conn

    def get(self, key):
        return key

    def name(self):
        return self.conn

    @staticmethod
    def same(value):
        return value
