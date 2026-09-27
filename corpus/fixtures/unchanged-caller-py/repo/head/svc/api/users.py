from svc import cache, db


def show_user(uid):
    user = cache.get_user(uid)
    if user is None:
        user = db.load_user(uid)
    return {"id": user.id, "email": user.email}
