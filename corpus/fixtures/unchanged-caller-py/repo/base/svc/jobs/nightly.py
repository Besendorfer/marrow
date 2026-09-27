from svc import cache, db, mailer


def send_digest(uids):
    for uid in uids:
        try:
            user = cache.get_user(uid)
        except KeyError:
            user = db.load_user(uid)
        mailer.send(user.email, "Your nightly digest")
