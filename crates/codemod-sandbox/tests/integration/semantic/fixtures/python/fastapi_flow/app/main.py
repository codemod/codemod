from fastapi import FastAPI

from .repository import OrderRepository

app = FastAPI()


def get_repo() -> OrderRepository:
    return OrderRepository(None)


@app.get("/orders")
def list_orders(q: str, repo: OrderRepository):
    return repo.search(q)


@app.post("/orders")
async def create_order(payload: dict):
    repo = get_repo()
    repo.save(payload)
    tags = [t.strip() for t in payload["tags"]]
    return {"tags": tags}
