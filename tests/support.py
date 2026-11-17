"""What the Python suites share: the stubs they run and the workflow they read.

Not a test file. The suites load it by path, the way they load the scripts they
test, because `tests/` is not a package and `scripts/run-python-tests.py`
imports each suite from its own path.
"""

import functools
import io
import os
import re
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = ROOT / ".github/workflows/check.yml"
STUBS = ROOT / "tests/stubs"

# A scratch repository must not read the developer's git configuration, nor be
# redirected by a GIT_DIR or GIT_INDEX_FILE inherited from a hook.
GIT_ENV = {
    **{k: v for k, v in os.environ.items() if not k.startswith("GIT_")},
    "GIT_CONFIG_GLOBAL": "/dev/null",
    "GIT_CONFIG_SYSTEM": "/dev/null",
}
COMMIT = "git -c user.name=t -c user.email=t@t commit -q"


def sh(script, cwd):
    """Run `script` under sh in `cwd` with the isolated git environment."""
    return subprocess.run(
        ["sh", "-c", script],
        cwd=cwd,
        capture_output=True,
        text=True,
        env=GIT_ENV,
        check=False,
    )


# The tools a script under test needs from the system, and nothing else: a PATH
# that holds only these cannot reach whatever toolchain the machine has.
CHECKER_TOOLS = ("sh", "sed", "grep", "tr", "od", "dirname", "cat", "wc", "uname")


def link_system_tools(directory, names=CHECKER_TOOLS):
    """Symlink each named system tool into directory; return what is missing."""
    missing = []
    for name in names:
        found = shutil.which(name)
        if found is None:
            missing.append(name)
        else:
            (Path(directory) / name).symlink_to(found)
    return missing


def stub(name, interpreter=None):
    """The source of a fake executable kept under tests/stubs/.

    Files there rather than strings in a suite, so each is read, parsed and
    linted as the language it is written in: inside a Python string a shell stub
    doubled every backslash and reached neither `sh -n` nor shellcheck.
    `interpreter` replaces the shebang, for a stub that has to run under the
    suite's own Python.
    """
    text = (STUBS / name).read_text()
    if interpreter is not None:
        text = f"#!{interpreter}\n" + text.split("\n", 1)[1]
    return text


def write_executable(path, text):
    """Write a stub a case will run, without this process ever holding it open.

    The runner runs cases on threads, and a file one thread has open for writing
    stays open in any process another thread forks until that process execs.
    Running the stub in that window fails with "Text file busy". `cp` writes it
    from a process of its own.
    """
    source = path.with_name(path.name + ".source")
    source.write_text(text)
    subprocess.run(["cp", str(source), str(path)], check=True)
    path.chmod(0o755)


def quiet_meter(memory):
    """Keep a loaded memory.py's progress and summary lines out of the gate log.

    They are the tool's output, not the suite's. A module loaded per case owns
    its METER, so the cases the runner runs at once do not share the sink.
    """
    memory.METER.stream = io.StringIO()
    memory.METER.say = lambda text, err=False: print(text, file=memory.METER.out())


# The workflow is read as text, because the gate's Python has no YAML library.
# Each helper names what it could not find, so a reshaped workflow fails with
# the job or step it lost rather than an IndexError.
def section(text, start, end, what):
    if start not in text:
        raise AssertionError(f"{what} is no longer in the workflow")
    return text.split(start, 1)[1].split(end, 1)[0]


def function_source(text, name):
    """A whole shell function of a script, as the script defines it."""
    body = section(text, f"\n{name}()\n{{\n", "\n}\n", f"function {name}")
    return f"{name}()\n{{\n{body}\n}}\n"


@functools.cache
def workflow_text():
    return WORKFLOW.read_text()


def job(name, text=None):
    """The body of one job, up to the next job."""
    text = workflow_text() if text is None else text
    body = section(text, f"\n  {name}:\n", "\0", f"job {name}")
    following = re.search(r"\n  [a-z][a-z0-9_-]*:\n", body)
    return body[: following.start()] if following else body


def step(text, name):
    return section(text, f"      - name: {name}\n", "\n      - ", f"step {name}")


def run_block(text):
    """The script of the first `run: |` in text, dedented to its key's indent.

    Read from the text rather than assumed, since a workflow step and a
    composite action's step sit at different depths.
    """
    key = re.search(r"^( *)run: \|\n", text, re.M)
    if key is None:
        raise AssertionError("run block is no longer in the workflow")
    width = len(key[1]) + 2
    body = []
    for line in text[key.end() :].splitlines():
        if line and not line.startswith(" " * width):
            break
        body.append(line[width:])
    return "\n".join(body)
