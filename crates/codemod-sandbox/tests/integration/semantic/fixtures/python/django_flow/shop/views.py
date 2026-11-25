from django.db import connection

from . import services
from .services import place_order


def search(request):
    q = request.GET["q"]
    cursor = connection.cursor()
    cursor.execute(f"SELECT * FROM orders WHERE name = '{q}'")
    return cursor.fetchall()


def checkout(request):
    order = place_order(request.user, request.POST.getlist("items"))
    return order


def history(request):
    return services.find_orders(request.user, None)
