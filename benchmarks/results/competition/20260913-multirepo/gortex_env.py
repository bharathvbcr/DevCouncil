#!/usr/bin/env python3
"""Write this run's private Gortex environment.

Its own socket, pidfile, logfile and XDG roots, so a daemon started here can
never attach to, or be confused with, the daemon the 20260913-48cd3c7 run left
configured.
"""

import json
from pathlib import Path

OUT = Path(__file__).resolve().parent
SCRATCH = Path(json.loads((OUT / "plan.json").read_text())["scratch"])

env = {
    "XDG_CONFIG_HOME": str(SCRATCH / "xdg/config"),
    "XDG_DATA_HOME": str(SCRATCH / "xdg/data"),
    "XDG_CACHE_HOME": str(SCRATCH / "xdg/cache"),
    "GORTEX_DAEMON_SOCKET": "/tmp/dmbench-multirepo.sock",
    "GORTEX_DAEMON_PIDFILE": str(SCRATCH / "gortex.pid"),
    "GORTEX_DAEMON_LOGFILE": str(SCRATCH / "gortex.log"),
    "DO_NOT_TRACK": "1",
}
for key in ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME"):
    Path(env[key]).mkdir(parents=True, exist_ok=True)

(OUT / "gortex-env.json").write_text(json.dumps(env, indent=2) + "\n")
print(json.dumps(env, indent=2))
