"""The interpreter the instructor tool runs under, found rather than named.

`task-package.py` needs Python 3.9 or newer, and `build` also needs the
`cryptography` package. Rather than asking the instructor to name an
interpreter, the tool reruns itself under one that has both: a newer
`python3.x` on PATH, or the tool's own environment in `target/task-tools`,
which it sets up from `scripts/requirements-task.txt` the first time `build`
needs it. Written for any Python 3, since it runs before the tool knows it has
a new enough one.
"""

import os
import shutil
import subprocess
import sys
from pathlib import Path

MINIMUM = (3, 9)
ROOT = Path(__file__).resolve().parents[2]
REQUIREMENTS = ROOT / "scripts" / "requirements-task.txt"
# Set on the rerun, so an interpreter that still cannot run the tool is
# reported rather than rerun again.
RERUN = "CODETRIAL_TASK_RUNTIME"
VERSIONED = ["python3.%d" % minor for minor in range(14, MINIMUM[1] - 1, -1)]


def tools():
    """The tool's own environment: `CODETRIAL_TASK_TOOLS`, or
    `target/task-tools` in this checkout."""
    return Path(
        os.environ.get("CODETRIAL_TASK_TOOLS") or ROOT / "target" / "task-tools"
    )


def _python(environment):
    return environment / ("Scripts/python.exe" if os.name == "nt" else "bin/python")


def _runs(python, code):
    try:
        return (
            subprocess.call(
                [str(python), "-c", code],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            == 0
        )
    except OSError:
        return False


def _new_enough(python):
    return _runs(python, "import sys; sys.exit(sys.version_info < %r)" % (MINIMUM,))


def _has_cryptography(python):
    return _runs(python, "import cryptography")


def _rerun(python):
    if os.environ.get(RERUN):
        raise SystemExit(
            "task-package: %s cannot run this tool either; install the packages"
            " in %s into a Python %d.%d or newer and run the tool with it"
            % (python, REQUIREMENTS, MINIMUM[0], MINIMUM[1])
        )
    os.environ[RERUN] = "1"
    argv = [str(python), str(Path(sys.argv[0]).resolve())] + sys.argv[1:]
    if os.name == "nt":
        raise SystemExit(subprocess.call(argv))
    os.execv(str(python), argv)


def _set_up(base, environment):
    sys.stderr.write(
        "task-package: setting up %s with the packages in %s; this happens"
        " once and downloads them\n" % (environment, REQUIREMENTS)
    )
    subprocess.check_call([str(base), "-m", "venv", "--clear", str(environment)])
    subprocess.check_call(
        [str(_python(environment)), "-m", "pip", "install", "--quiet"]
        + ["--disable-pip-version-check", "-r", str(REQUIREMENTS)]
    )


def ensure(needs_cryptography):
    """Returns when this interpreter can run the tool, and otherwise reruns
    the tool under one that can, setting one up if `build` needs it."""
    here_new = sys.version_info >= MINIMUM
    if here_new and (not needs_cryptography or _has_cryptography(sys.executable)):
        return
    environment = tools()
    python = _python(environment)
    if (
        python.exists()
        and _new_enough(python)
        and (not needs_cryptography or _has_cryptography(python))
    ):
        _rerun(python)
    base = sys.executable if here_new else None
    if base is None:
        base = next(
            (
                found
                for found in map(shutil.which, ["python3"] + VERSIONED)
                if found and _new_enough(found)
            ),
            None,
        )
    if base is None:
        raise SystemExit(
            "task-package: needs Python %d.%d or newer, and none is on PATH" % MINIMUM
        )
    if not needs_cryptography:
        _rerun(base)
    if os.environ.get(RERUN):
        _rerun(python)
    _set_up(base, environment)
    _rerun(python)
