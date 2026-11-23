#!/usr/bin/env python3
"""PATH-injected shims shared by the execution tests.

`install(base)` builds a throwaway tree::

    base/path/bitbake, base/path/devtool     plain shims
    base/init/bitbake, base/init/devtool     shims revealed only by sourcing
    base/oe-init-build-env                   stand-in Yocto init script
    base/build, base/other-build             empty build dirs
    base/home                                empty HOME

The shims echo their argv and cwd, so a test can assert the exact command
line and working directory; a command line containing `flood` also emits
FLOOD_LINES stdout lines so the over-cap path (output to the log file) can be
tested without a Yocto build. An argument `fixture-<stamp>` prints the body of
the captured real log `tests/fixtures/logs/<stamp>-*.log` and exits with that
log's exit code, so the digest can be tested on real bitbake output. The
stand-in init script prepends `base/init` to PATH the way the real
`oe-init-build-env` prepends bitbake, so a command that resolved to
`init-path bitbake` proves the environment was sourced first.

Nothing here is written outside `base`, which the caller owns (and removes).
"""
import os

FIXTURE_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "tests", "fixtures", "logs")

_INIT_SCRIPT = """#!/bin/sh
# $1 is the build dir the server passes through ('.' = the command's own cwd).
echo "INIT-SOURCED builddir='$1'"
export PATH="{init_dir}:$PATH"
"""

_SHIM = """#!/bin/sh
echo "{tag}{tool}-args:$*"
echo "{tag}{tool}-cwd:$(pwd)"
# `flood` in the command line emits FLOOD_LINES stdout lines (default 200), so a
# test can drive the over-cap path that sends output to the log file instead of
# the result; `floodfail` additionally exits 3.
case "$*" in
  *flood*)
    i=0
    while [ "$i" -lt "${{FLOOD_LINES:-200}}" ]; do
      echo "{tag}{tool}-flood:$i"
      i=$((i + 1))
    done
    ;;
esac
case "$*" in
  *floodfail*) exit 3 ;;
esac
# `fixture-<stamp>` replays a captured real log (body after its 3-line header)
# and exits with the exit code recorded in that header.
for a in "$@"; do
  case "$a" in
    fixture-*)
      f=$(ls "$FIXTURE_DIR/${{a#fixture-}}"-*.log)
      tail -n +4 "$f"
      sed -n 3p "$f" | grep -Eq 'exit_code: 0( |$)' && exit 0
      exit 1
      ;;
  esac
done
exit 0
"""


def _write(path, body):
    with open(path, "w") as fh:
        fh.write(body)
    os.chmod(path, 0o755)


def install(base):
    """Create the shim tree under `base` and return its paths as a dict."""
    paths = {
        "path_dir": os.path.join(base, "path"),
        "init_dir": os.path.join(base, "init"),
        "build_dir": os.path.join(base, "build"),
        "other_build_dir": os.path.join(base, "other-build"),
        "home": os.path.join(base, "home"),
        "init_script": os.path.join(base, "oe-init-build-env"),
    }
    for key in ("path_dir", "init_dir", "build_dir", "other_build_dir", "home"):
        os.makedirs(paths[key], exist_ok=True)

    for directory, tag in ((paths["path_dir"], ""), (paths["init_dir"], "init-path ")):
        for tool in ("bitbake", "devtool"):
            _write(os.path.join(directory, tool), _SHIM.format(tag=tag, tool=tool))

    _write(paths["init_script"], _INIT_SCRIPT.format(init_dir=paths["init_dir"]))
    return paths


def server_env(paths):
    """Env for the server process: shims on PATH, empty HOME, no host config."""
    env = dict(os.environ)
    env["PATH"] = paths["path_dir"] + os.pathsep + env.get("PATH", "")
    env["HOME"] = paths["home"]
    env["FIXTURE_DIR"] = FIXTURE_DIR
    return env
