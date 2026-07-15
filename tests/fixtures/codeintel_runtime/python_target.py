import time


def child(value: int) -> int:
    result = value + 1
    time.sleep(0.05)
    return result


def parent() -> int:
    return child(41)


print(parent(), flush=True)
