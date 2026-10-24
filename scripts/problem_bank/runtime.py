"""The interpreter the instructor tool runs under, found rather than named.

`task-package.py` needs Python 3.9 or newer, and `build` also needs the
`cryptography` package. Rather than asking the instructor to name an
interpreter, the tool reruns itself under one that has both: a newer
`python3.x` on PATH, or the tool's own environment in `target/task-tools`,
which it sets up from `scripts/requirements-task.txt` the first time `build`
needs it. Written for any Python 3, since it runs before the tool knows it has
a new enough one.
"""

import importlib.util
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
    # Creates, never clears. CODETRIAL_TASK_TOOLS can name any directory, and
    # `venv --clear` once emptied a system Python's prefix a test pointed it at.
    # A dangling symlink is refused too: venv would write through it.
    if environment.exists() or environment.is_symlink():
        raise SystemExit(
            "task-package: %s exists but cannot run `build`; remove it to have"
            " it set up again, or install the packages in %s into it"
            % (environment, REQUIREMENTS)
        )
    sys.stderr.write(
        "task-package: setting up %s with the packages in %s; this happens"
        " once and downloads them\n" % (environment, REQUIREMENTS)
    )
    try:
        subprocess.check_call([str(base), "-m", "venv", str(environment)])
        subprocess.check_call(
            [str(_python(environment)), "-m", "pip", "install", "--quiet"]
            + ["--disable-pip-version-check", "-r", str(REQUIREMENTS)]
        )
    except BaseException:
        # Only what this call created goes: a download that failed halfway
        # would otherwise leave a directory every later run refuses.
        shutil.rmtree(str(environment), ignore_errors=True)
        raise


def ensure(needs_cryptography):
    """Returns when this interpreter can run the tool, and otherwise reruns
    the tool under one that can, setting one up if `build` needs it."""
    here_new = sys.version_info >= MINIMUM
    # This interpreter is asked in process; only another one costs a process.
    if here_new and (
        not needs_cryptography or importlib.util.find_spec("cryptography") is not None
    ):
        return
    environment = tools()
    python = _python(environment)
    if (
        python.exists()
        and _new_enough(python)
        and (not needs_cryptography or _has_cryptography(python))
    ):
        _rerun(python)
    found = [
        path
        for path in map(shutil.which, ["python3"] + VERSIONED)
        if path and _new_enough(path)
    ]
    # One already on PATH that has `cryptography` saves setting one up.
    if needs_cryptography and not os.environ.get(RERUN):
        ready = next((path for path in found if _has_cryptography(path)), None)
        if ready is not None:
            _rerun(ready)
    base = sys.executable if here_new else next(iter(found), None)
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


# `scripts/test.sh` sets the environment up before its suites run in parallel,
# through this one path rather than a copy of it.
if __name__ == "__main__":
    _set_up(Path(sys.executable), tools())
