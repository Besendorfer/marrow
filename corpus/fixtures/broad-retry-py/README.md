# broad-retry-py

Hard fixture (corpus v7; `repo/` snapshot). The retry decorator widens from
TransientError to any exception. The damage is outside the diff:
`svc/orders.py` retries `create_order`, which charges a card (a
non-idempotent POST in `svc/payments.py`) before code that can raise. A
retry after a successful charge charges again. Needs reading two unchanged
files.
