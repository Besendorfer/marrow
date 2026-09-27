"""In-process user cache."""

_USERS = {}


def put_user(user):
    _USERS[user.id] = user


def get_user(uid):
    """Return the cached user; raises KeyError on a miss."""
    return _USERS[uid]
