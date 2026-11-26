from lib import Repo, audit, by_name, collect, first, identity, options, save


def local(text):
    return text.strip()


def from_a(request):
    a = identity(request.args)
    sink(a)


def from_b():
    b = identity("b-literal")
    log(b)


def passes(user):
    audit(user)
    kept = first(user, 1)
    dropped = first(1, user)
    named = by_name(1, b=user)
    unnamed = by_name(user, b=2)
    packed = collect(1, user)
    keywords = options(x=user)
    repo = Repo(user)
    got = repo.get(user)
    same = Repo.same(user)
    save(user)
    trimmed = local(user)
