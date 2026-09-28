import time
from functools import wraps

from svc.errors import TransientError


def retry(times=3, delay=0.5):
    """Retry a call that failed with a transient error."""
    def deco(fn):
        @wraps(fn)
        def wrapper(*args, **kwargs):
            for attempt in range(times):
                try:
                    return fn(*args, **kwargs)
                except TransientError:
                    if attempt == times - 1:
                        raise
                    time.sleep(delay * (attempt + 1))
        return wrapper
    return deco
