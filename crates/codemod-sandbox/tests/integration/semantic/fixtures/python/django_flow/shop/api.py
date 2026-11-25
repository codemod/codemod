from rest_framework import viewsets

from shop.repo import OrderRepository
from shop.tasks import notify


class OrderViewSet(viewsets.ViewSet):
    def list(self, request):
        repo = OrderRepository(None)
        return self.respond(repo.find(request.query_params["customer"]))

    def respond(self, rows):
        notify.delay(rows, 1)
        return rows
