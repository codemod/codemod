from sqlalchemy import text

from app import Session


class OrderRepository:
    session: Session

    def __init__(self, session: Session):
        self.session = session

    def search(self, term: str) -> list:
        return self.session.execute(text(f"SELECT * FROM orders WHERE name = '{term}'"))

    def save(self, order):
        self.session.add(order)
        return self.validate(order)

    def validate(self, order):
        return order
