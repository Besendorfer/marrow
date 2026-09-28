import requests

API = "https://payments.internal/v1"


def charge(amount, card):
    """POST a charge. Not idempotent: each call charges the card."""
    return requests.post(f"{API}/charges", json={"amount": amount, "card": card}, timeout=10)
