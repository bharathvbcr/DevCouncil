"""Entry module.

Deliberately thin. Every symbol whose liveness this corpus measures lives in
`api.py` or `helpers.py`, because a `__main__` guard makes this file a
`ScriptEntry` and exempts *everything* in it — so dead code placed here would be
masked by the file-level exemption rather than by the verdict under test.
"""

from api import Api

if __name__ == "__main__":
    Api().public_op()
