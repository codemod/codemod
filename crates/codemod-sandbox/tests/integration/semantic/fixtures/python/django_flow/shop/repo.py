class OrderRepository:
    def __init__(self, cursor):
        self.cursor = cursor

    def find(self, customer):
        self.cursor.execute("SELECT * FROM orders WHERE customer = %s", [customer])
        return self.cursor.fetchall()
