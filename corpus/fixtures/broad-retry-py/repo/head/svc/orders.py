from svc import db, payments
from svc.models import Order
from svc.retry import retry


@retry(times=3)
def create_order(cart):
    resp = payments.charge(cart.total, cart.card)  # charges the card
    order = Order(id=resp.json()["order_id"])
    db.save(order)
    return order
