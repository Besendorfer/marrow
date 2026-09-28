import time
from functools import wraps



def retry(times=3, delay=0.5):
    """Retry a call that failed. Transient failures surface as many exception
    types (timeouts, 502s raised as ValueError by the JSON parser), so retry
    on any exception."""
    def deco(fn):
        @wraps(fn)
        def wrapper(*args, **kwargs):
            for attempt in range(times):
                try:
                    return fn(*args, **kwargs)
                except Exception:
                    if attempt == times - 1:
                        raise
                    time.sleep(delay * (attempt + 1))
        return wrapper
    return deco
