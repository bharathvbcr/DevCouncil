from patch import PatchError, ProtocolError


def check(value):
    if isinstance(value, ProtocolError):
        return False
    try:
        raise PatchError("boom")
    except PatchError:
        return True
