from .repo import OrderRepository


def find_orders(customer, repo: OrderRepository):
    return repo.find(customer)


def place_order(customer, items):
    lines = []
    for item in items:
        lines.append(item)
    lines.sort()
    count = len(lines)
    return {"customer": customer, "count": count}
