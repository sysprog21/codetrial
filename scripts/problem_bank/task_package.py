"""Instructor packaging of structurally validated task sets; no publication."""

from __future__ import annotations

import base64
import hashlib
import hmac
import json
import re
import os
from pathlib import Path

from .bank import (
    ROOT,
    read_json,
    validated_problem_data,
    validated_problems,
    write_json,
)
from .emit import candidate_problem
from .rules import validated_variant
from .task_structure import structural
from .tasks import DEFAULTS, VARIABLE, identifier, validated_sidecars

PACKAGE_KEYS = {
    "packageVersion",
    "setId",
    "setVersion",
    "rules",
    "problems",
    "judges",
    "variants",
    "sidecars",
}
MANIFEST_KEYS = {
    "packageVersion",
    "setId",
    "setVersion",
    "salt",
    "ciphertextSha256",
    "ciphertextBytes",
}
RULE_KEYS = {"durationMin", "maxHintRungs", "closesAt", "lookAwaySeconds", "rulesNote"}
# `MIN_DURATION_MIN` and `MAX_DURATION_MIN` in src/config.rs.
DURATION_BOUNDS = (10, 90)
# Files a publish directory may hold besides each version's package.
SITE_FILES = {".nojekyll", "index.html", "CNAME", "README.md"}
VERSION_FILES = {"manifest.json", "tasks.enc", "index.html"}


def compact(value):
    return json.dumps(
        value, ensure_ascii=False, separators=(",", ":"), sort_keys=True
    ).encode("utf-8")


def metadata(set_id, version):
    identifier(set_id, "setId")
    if type(version) is not int or not 1 <= version <= 2147483647:
        raise ValueError("invalid setVersion")


def associated_data(set_id, version):
    metadata(set_id, version)
    return compact(["codetrial-task-set-v1", set_id, version])


def checked_pin(pin):
    if (
        not isinstance(pin, str)
        or len(pin) != DEFAULTS["pinDigits"]
        or not pin.isascii()
        or not pin.isdigit()
    ):
        raise ValueError(f"PIN must contain {DEFAULTS['pinDigits']} ASCII digits")
    return pin


def derive_key(pin, set_id, version, salt):
    """The package key: PBKDF2-HMAC-SHA256 of the PIN, salted per version."""
    from cryptography.hazmat.primitives import hashes
    from cryptography.hazmat.primitives.kdf.pbkdf2 import PBKDF2HMAC

    metadata(set_id, version)
    return PBKDF2HMAC(
        algorithm=hashes.SHA256(),
        length=32,
        salt=compact(["codetrial-task-kdf-v1", set_id, version, salt]),
        iterations=DEFAULTS["kdfIterations"],
    ).derive(checked_pin(pin).encode("ascii"))


def validated_rules(raw):
    """The complete set rules: `task-set.json` over the defaults."""
    if raw is None:
        raw = {}
    if not isinstance(raw, dict) or not set(raw) <= RULE_KEYS:
        raise ValueError("task-set.json has unknown fields")
    rules = {
        "durationMin": DEFAULTS["durationMin"],
        "maxHintRungs": DEFAULTS["maxHintRungs"],
        "closesAt": None,
        "lookAwaySeconds": DEFAULTS["lookAwaySeconds"],
        "rulesNote": None,
        **raw,
    }
    low, high = DURATION_BOUNDS
    for key, bounds in (
        ("durationMin", (low, high)),
        ("maxHintRungs", (0, DEFAULTS["maxHintRungs"])),
        ("lookAwaySeconds", (5, 60)),
    ):
        value = rules[key]
        if type(value) is not int or not bounds[0] <= value <= bounds[1]:
            raise ValueError(f"{key} must be an integer in {bounds[0]}..{bounds[1]}")
    closes = rules["closesAt"]
    if closes is not None and not valid_close(closes):
        raise ValueError("closesAt must be an RFC 3339 UTC time ending in Z")
    note = rules["rulesNote"]
    if note is not None and (
        not isinstance(note, str)
        or not note.strip()
        or len(note.encode("utf-8")) > 1024
    ):
        raise ValueError("rulesNote must be nonblank text of at most 1024 bytes")
    return rules


def validate_package(package):
    # The limits CodeTrial applies when it opens the package; a lower one here
    # would only refuse what the learner's CodeTrial accepts.
    task_bytes, set_bytes = DEFAULTS["taskBytes"], DEFAULTS["setBytes"]
    if (
        not isinstance(package, dict)
        or set(package) != PACKAGE_KEYS
        or type(package["packageVersion"]) is not int
        or package["packageVersion"] != 1
    ):
        raise ValueError("invalid task package schema")
    metadata(package["setId"], package["setVersion"])
    if validated_rules(package["rules"]) != package["rules"]:
        raise ValueError("package rules are not resolved")
    if len(compact(package)) > set_bytes:
        raise ValueError("task set exceeds size limit")
    problems = package["problems"]
    if not isinstance(problems, list) or not problems:
        raise ValueError("empty task set")
    ids = [row.get("id") for row in problems if isinstance(row, dict)]
    if (
        len(ids) != len(problems)
        or any(
            not isinstance(value, str)
            or not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,63}", value)
            for value in ids
        )
        or len(set(ids)) != len(ids)
    ):
        raise ValueError("invalid or duplicate task ids")
    for key in ("judges", "variants"):
        if not isinstance(package[key], dict) or set(package[key]) != set(ids):
            raise ValueError(f"{key} must match task ids exactly")
    entries = {}
    for problem in problems:
        task_id = problem["id"]
        structural(problem, package["judges"][task_id], package["variants"][task_id])
        entries[task_id] = {
            **validated_variant(
                problem, package["judges"][task_id], package["variants"][task_id]
            ),
            "variant": package["variants"][task_id],
        }
        row = {
            "problem": problem,
            "judge": package["judges"][task_id],
            "variant": package["variants"][task_id],
            "sidecar": package["sidecars"].get(task_id)
            if isinstance(package["sidecars"], dict)
            else None,
        }
        if len(compact(row)) > task_bytes:
            raise ValueError("task exceeds size limit")
    validated_problem_data(problems)
    sidecars = validated_sidecars(
        problems, package["judges"], package["variants"], package["sidecars"]
    )
    return entries, sidecars


def load_bank(directory, set_id, version):
    directory = Path(directory)
    package = {
        "packageVersion": 1,
        "setId": set_id,
        "setVersion": version,
        "rules": validated_rules(
            read_json(directory / "task-set.json")
            if (directory / "task-set.json").exists()
            else None
        ),
        "problems": validated_problems(directory / "problems.json"),
        "judges": read_json(directory / "judges.json"),
        "variants": read_json(directory / "variants.json"),
        "sidecars": read_json(directory / "task-mode.json")
        if (directory / "task-mode.json").exists()
        else {},
    }
    validate_package(package)
    return package


def projection(entry, sidecar):
    variant = entry["variant"]
    return {
        "title": variant["title"],
        "brief": variant["brief"],
        "contract": variant["contract"],
        "clarifications": variant["clarifications"],
        "hints": variant["hints"][: sidecar["maxHintRungs"]],
        "completionTargets": sidecar["completionTargets"],
        "understandingChecks": sidecar["understandingChecks"],
    }


def valid_close(text):
    """Whether `text` is a real UTC time in the one form the server parses."""
    from datetime import datetime

    # ASCII digits only: `\d` and `strptime` take other scripts' digits too.
    if not isinstance(text, str) or not re.fullmatch(
        r"[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z", text
    ):
        return False
    try:
        parsed = datetime.strptime(text, "%Y-%m-%dT%H:%M:%SZ")
    except ValueError:
        return False
    # The server counts in seconds since 1970 and refuses anything earlier.
    return parsed.year >= 1970


def validated_task(package, task_id):
    """The validated entry and sidecar of one task in `package`, with the
    hint maximum the set's rules lower it to, as the server applies it."""
    entries, sidecars = validate_package(package)
    if task_id not in entries:
        raise ValueError("unknown task")
    sidecar = dict(sidecars[task_id])
    ceiling = (package.get("rules") or {}).get("maxHintRungs")
    if ceiling is not None:
        sidecar["maxHintRungs"] = min(sidecar["maxHintRungs"], ceiling)
    return entries[task_id], sidecar


def render_prompt(package, task_id):
    entry, sidecar = validated_task(package, task_id)
    values = {
        "title": entry["variant"]["title"],
        "language": "python",
        "target": "",
        "phase": "ready",
    }
    instructor = VARIABLE.sub(
        lambda match: values[match.group(1)], sidecar["interactionPrompt"]
    )
    public = projection(entry, sidecar)
    public.pop("hints")
    public["hintRungsMax"] = sidecar["maxHintRungs"]
    template = (ROOT / "problem-bank/task-prompt.txt").read_text()
    substitutions = {
        "phase": "ready",
        "instructorPrompt": instructor,
        "publicProjection": compact(public).decode(),
        "code": json.dumps("", ensure_ascii=False),
    }
    return VARIABLE.sub(lambda match: substitutions[match.group(1)], template)


def preview(package, task_id):
    entry, sidecar = validated_task(package, task_id)
    return {
        "problem": candidate_problem(entry),
        "rules": sidecar,
        "judge": entry["judge"],
        "resultsLabel": "Learner-reported practice evidence",
    }


def encrypt(package, pin):
    """`tasks.enc` and its manifest, keyed by `pin` under a fresh salt."""
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM

    validate_package(package)
    salt = base64.b64encode(os.urandom(16)).decode("ascii")
    key = derive_key(pin, package["setId"], package["setVersion"], salt)
    nonce = os.urandom(12)
    ciphertext = nonce + AESGCM(key).encrypt(
        nonce,
        compact(package),
        associated_data(package["setId"], package["setVersion"]),
    )
    manifest = {
        "packageVersion": 1,
        "setId": package["setId"],
        "setVersion": package["setVersion"],
        "salt": salt,
        "ciphertextSha256": hashlib.sha256(ciphertext).hexdigest(),
        "ciphertextBytes": len(ciphertext),
    }
    return ciphertext, manifest


def checked_manifest(manifest):
    if (
        not isinstance(manifest, dict)
        or set(manifest) != MANIFEST_KEYS
        or type(manifest["packageVersion"]) is not int
        or manifest["packageVersion"] != 1
        or type(manifest["ciphertextBytes"]) is not int
        or not isinstance(manifest["salt"], str)
        or len(base64.b64decode(manifest["salt"], validate=True)) != 16
    ):
        raise ValueError("invalid manifest schema")
    metadata(manifest["setId"], manifest["setVersion"])
    return manifest


def decrypt(ciphertext, manifest, pin):
    from cryptography.hazmat.primitives.ciphers.aead import AESGCM

    checked_manifest(manifest)
    if (
        manifest["ciphertextBytes"] != len(ciphertext)
        or not 28 <= len(ciphertext) <= DEFAULTS["setBytes"] + 28
    ):
        raise ValueError("invalid ciphertext size")
    if not hmac.compare_digest(
        hashlib.sha256(ciphertext).hexdigest(), str(manifest["ciphertextSha256"])
    ):
        raise ValueError("ciphertext hash mismatch")
    key = derive_key(pin, manifest["setId"], manifest["setVersion"], manifest["salt"])
    plaintext = AESGCM(key).decrypt(
        ciphertext[:12],
        ciphertext[12:],
        associated_data(manifest["setId"], manifest["setVersion"]),
    )
    package = json.loads(plaintext)
    validate_package(package)
    if (
        package["setId"] != manifest["setId"]
        or package["setVersion"] != manifest["setVersion"]
    ):
        raise ValueError("authenticated metadata mismatch")
    return package


def site_base(site):
    """The HTTPS base URL `sets/` lives under, without a trailing slash."""
    from urllib.parse import urlsplit

    parts = urlsplit(site or "")
    try:
        # `urlsplit` checks a port only when asked for it; CodeTrial refuses a
        # bad one, so the links built from this must not carry one either.
        parts.port
    except ValueError:
        raise ValueError(
            "--site must be an https URL without credentials or query"
        ) from None
    if (
        parts.scheme != "https"
        or not parts.hostname
        or parts.username
        or parts.password
        or parts.query
        or parts.fragment
    ):
        raise ValueError("--site must be an https URL without credentials or query")
    return site.rstrip("/")


def assignment_link(site, set_id, task_id, version):
    from urllib.parse import quote, urlencode

    query = urlencode({"site": site, "version": version})
    return f"http://localhost:3000/t/{quote(set_id)}/{quote(task_id)}?{query}"


def rules_in_words(rules):
    """The set's rules as the landing page and the learner read them."""
    lines = [
        f"You have {rules['durationMin']} minutes once you press Start.",
        "Stay in fullscreen for the whole attempt: leaving it ends the attempt"
        " at once and marks it invalid.",
        "Keep this page visible: switching to another tab or window, minimizing"
        " it or locking the screen ends the attempt at once and marks it invalid.",
        "Keep your face in front of the camera. Looking away or down, or"
        f" leaving the frame, for more than {rules['lookAwaySeconds']} seconds"
        " in a row ends the attempt and marks it invalid; a glance at the"
        " keyboard is fine. A countdown and a sound warn you first.",
        "Use the calculator inside the workspace instead of another app.",
        "Turn off notifications and keep your machine plugged in before you start.",
    ]
    if rules["closesAt"]:
        lines.append(f"Attempts must start before {rules['closesAt']}.")
    if rules["rulesNote"]:
        lines.append(rules["rulesNote"])
    return lines


def version_page(package, site):
    from html import escape

    entries, _ = validate_package(package)
    set_id, version = package["setId"], package["setVersion"]
    tasks = "\n".join(
        f"<li><strong>{escape(entry['variant']['title'])}</strong>: "
        f'<a href="{escape(link)}">Open in CodeTrial</a>'
        f"<br><code>{escape(link)}</code></li>"
        for task_id, entry in sorted(entries.items())
        for link in [assignment_link(site, set_id, task_id, version)]
    )
    rules = "\n".join(
        f"<li>{escape(line)}</li>" for line in rules_in_words(package["rules"])
    )
    return f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{escape(set_id)} version {version}</title></head><body>
<h1>{escape(set_id)}, version {version}</h1>
<h2>Before you start</h2>
<p>You need CodeTrial running on this computer, your own Gemini API key and
LiveKit credentials, a GitHub account, a microphone and a camera. Your
instructor gives you the PIN separately.</p>
<h2>Rules</h2>
<ul>
{rules}
</ul>
<h2>Tasks</h2>
<p>If a link does not open, start CodeTrial and open the address shown under
it. If CodeTrial is not on port 3000, change 3000 in the address to its
port.</p>
<ul>
{tasks}
</ul>
</body></html>
"""


def site_page(directory):
    from html import escape

    versions = sorted(
        (path.parent.parent.name, int(path.parent.name))
        for path in Path(directory).glob("sets/*/*/manifest.json")
    )
    items = "\n".join(
        f'<li><a href="sets/{escape(set_id)}/{version}/">'
        f"{escape(set_id)}, version {version}</a></li>"
        for set_id, version in versions
    )
    return f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>CodeTrial assignments</title></head><body>
<h1>CodeTrial assignments</h1>
<ul>
{items}
</ul>
</body></html>
"""


def build(bank, set_id, version, pin, site, output):
    """Validate, encrypt, prove the round trip, write and scan one version."""
    return build_package(load_bank(bank, set_id, version), pin, site, output)


def build_package(package, pin, site, output):
    """`build` for a package already loaded and validated, so a caller can
    check it before asking for the PIN and report on exactly what was built."""
    site = site_base(site)
    set_id, version = package["setId"], package["setVersion"]
    ciphertext, manifest = encrypt(package, pin)
    if decrypt(ciphertext, manifest, pin) != package:
        raise RuntimeError("encrypted package did not decrypt to its source")
    output = Path(output)
    directory = output / "sets" / set_id / str(version)
    directory.mkdir(parents=True, exist_ok=False)
    (directory / "tasks.enc").write_bytes(ciphertext)
    write_json(directory / "manifest.json", manifest)
    (directory / "index.html").write_text(version_page(package, site))
    (output / ".nojekyll").write_text("")
    (output / "index.html").write_text(site_page(output))
    publish_scan(output)
    return directory


def publish_scan(directory):
    """Refuse anything in a publish directory that is not public by design."""
    directory = Path(directory)
    manifests = []
    for path in directory.rglob("*"):
        if path.is_symlink():
            raise ValueError("publish directory contains a symlink")
        if path.is_dir():
            continue
        relative = path.relative_to(directory)
        if len(relative.parts) == 1:
            if path.name not in SITE_FILES:
                raise ValueError(f"publish directory contains {relative}")
        elif (
            len(relative.parts) != 4
            or relative.parts[0] != "sets"
            or path.name not in VERSION_FILES
        ):
            raise ValueError(f"publish directory contains {relative}")
        elif path.name == "manifest.json":
            manifests.append(path)
    if not manifests:
        raise ValueError("publish directory contains no package")
    for path in manifests:
        manifest = checked_manifest(read_json(path))
        expected = (
            directory
            / "sets"
            / manifest["setId"]
            / str(manifest["setVersion"])
            / "manifest.json"
        )
        if path != expected:
            raise ValueError("package outside versioned asset path")
        ciphertext = path.with_name("tasks.enc")
        if not ciphertext.is_file():
            raise ValueError("manifest without its ciphertext")
        data = ciphertext.read_bytes()
        if (
            len(data) != manifest["ciphertextBytes"]
            or not 28 <= len(data) <= DEFAULTS["setBytes"] + 28
            or hashlib.sha256(data).hexdigest() != manifest["ciphertextSha256"]
        ):
            raise ValueError("invalid published ciphertext")
    for path in directory.glob("sets/*/*/tasks.enc"):
        if not path.with_name("manifest.json").is_file():
            raise ValueError("ciphertext without its manifest")
    return len(manifests)
