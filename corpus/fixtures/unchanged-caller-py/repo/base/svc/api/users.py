from svc import cache, db


def show_user(uid):
    try:
        user = cache.get_user(uid)
    except KeyError:
        user = db.load_user(uid)
    return {"id": user.id, "email": user.email}
