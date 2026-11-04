#!/usr/bin/env python3
"""Refuse an agent's shell command that would get around the git hooks.

Registered as a Claude Code PreToolUse hook for the Bash tool (see
.claude/settings.json). It reads the tool call as JSON on standard input and
exits 2 with the reason on standard error, which Claude Code hands back to the
agent; any other exit lets the command run.

What it stops is a command that skips or sidesteps the hooks: --no-verify (in
any spelling git accepts), pointing core.hooksPath elsewhere, defining an alias
to hide a verb, building a commit by hand with commit-tree or update-ref, and
rewriting a hook or the script behind one. Those are the ways a commit or push
avoids the Change-Id and the review check on this machine.

It also refuses to read a per-developer credential file under config/ (a
`codetrial.env.<name>` other than the checked-in `.example`), a glob over them,
or the directory itself, with a shell reader such as cat, grep or cp. Tools that consume those files, `make check` and the
scripts that load them, are unaffected: it is the agent displaying a secret in
its own transcript that is refused.

The command line is parsed, not searched: shell quoting, `git -C`/`-c` global
options, option abbreviations, `sh -c`, `eval`, `xargs`, command substitution
and heredocs that feed a shell are all read. What it does not read is code in
another language (a Python heredoc calling os.system), a script written and
then run, or anything built at run time.

This is a speed bump for an agent that follows its tool permissions, not a
security boundary. A person, or an agent with another way to run a process,
can still bypass local hooks, which is why CI recomputes the chain and the
size of the change. See docs/development.md, "What cannot be enforced locally".
"""

import json
import os
import posixpath
import re
import shlex
import sys

# The verbs whose hooks --no-verify skips; only `commit` also has a short -n.
NO_VERIFY_VERBS = {"commit", "push", "merge", "rebase", "cherry-pick", "am", "revert"}
DESTINATION_LAST = {"cp", "install", "ln", "tee", "truncate", "dd"}
HOOKS_PATH = "points core.hooksPath away from this repository's hooks"
ALIAS = "defines a git alias, which can hide the verb being run"
HOOKS_CHANGE = "changes the installed git hooks or the script behind one"
SHELLS = {"sh", "bash", "zsh", "dash", "ksh"}
WRAPPERS = {
    "command",
    "sudo",
    "env",
    "nice",
    "time",
    "nohup",
    "exec",
    "xargs",
    "builtin",
}
# Options of `git commit` that take the next word as their value, so that word
# is a message or a path and says nothing about flags.
COMMIT_VALUES = {
    "-m", "-F", "-C", "-c", "-t", "-u", "--message", "--file", "--author", "--date",
    "--cleanup", "--reuse-message", "--reedit-message", "--template", "--fixup",
    "--squash", "--trailer", "--pathspec-from-file", "--gpg-sign", "--untracked-files",
}  # fmt: skip
READS = {
    "--get", "--get-all", "--get-regexp", "--get-urlmatch", "--unset", "--unset-all",
    "--remove-section", "--list", "-l", "--show-origin", "--show-scope", "--name-only",
}  # fmt: skip
CREDENTIAL_FILE = re.compile(r"(^|/)config/[^/]*\.env\.(?!example$)[^/]+$")
# A glob or the directory itself reaches the same files, and a recursive reader
# prints them: `grep -r KEY config`, `cat config/*.local`.
CREDENTIAL_REACH = re.compile(r"(?!.*\.example$)(^|/)config/[^/]*[*?\[][^/]*$")
CREDENTIAL_READ = (
    "reads a credential file under config/, which would put a secret in the transcript"
)
READERS = {
    "cat", "tac", "less", "more", "head", "tail", "grep", "egrep", "fgrep", "rg",
    "ag", "awk", "sed", "cut", "sort", "uniq", "tr", "od", "xxd", "hexdump",
    "strings", "base64", "bat", "cp", "mv", "scp", "rsync",
    "nl", "paste", "diff", "cmp", "jq", "tar", "zip", "zcat", "dd", "find",
}  # fmt: skip
# The last operand of these is where the data goes, not what is read, so a copy
# that creates a credential file (the README's setup step) is not a read.
TO_LAST = {"cp", "mv", "scp", "rsync"}
# Searchers that can be asked for names or counts instead of lines (`rg --files
# config`, `grep -rl KEY config`), which reveal no secret.
SEARCHERS = {"grep", "egrep", "fgrep", "rg", "ag"}
VALUE_OPTIONS = {"-e", "-f", "-m", "-A", "-B", "-C", "-g", "--regexp", "--file"}
NAME_FLAGS = {
    "--files", "--files-with-matches", "--files-without-match", "--count", "--quiet",
}  # fmt: skip
HOOK_FILE = re.compile(r"(^|/)\.git/hooks(/|$)|(^|/)scripts/git-[^/]+$")
WRITERS = {"rm", "mv", "cp", "ln", "chmod", "tee", "truncate", "install", "dd"}
# `<<<` is a here-string with no body, so it must not open a heredoc that would
# swallow the commands on the lines after it.
HEREDOC = re.compile(r"(?<!<)<<(?!<)-?\s*(['\"]?)(\w+)\1")
SUBSTITUTION = re.compile(r"\$\(([^()]*)\)|`([^`]*)`")


MIN_ABBREVIATION = len("--no-v")


def abbreviates(token, option):
    """Whether git would read `token` as `option`: an exact or unique prefix."""
    name = token.split("=", 1)[0]
    return len(name) >= MIN_ABBREVIATION and option.startswith(name)


def lines(text):
    """The command texts in a script: heredoc bodies are data unless a shell reads them."""
    out, pending, body = [], None, []

    def expand():
        # An unquoted body is expanded, so a $(...) in it runs, across lines too.
        for found in SUBSTITUTION.findall("\n".join(body)):
            out.extend((found[0] or found[1]).split("\n"))
        body.clear()

    rows, at = text.split("\n"), 0
    while at < len(rows):
        line = rows[at]
        at += 1
        if pending:
            word, mode = pending
            if line.strip() == word:
                expand()
                pending = None
            elif mode == "shell":
                # A shell reading the body deletes backslash-newline too; a
                # quoted body that is only data keeps its lines as written, so a
                # trailing backslash cannot swallow the delimiter after it.
                while (
                    line.endswith("\\") and at < len(rows) and rows[at].strip() != word
                ):
                    line = line[:-1] + rows[at]
                    at += 1
                out.append(line)
            elif mode == "expanded":
                body.append(line)
            continue
        # A backslash-newline is deleted by the shell, so `--no-\<newline>verify`
        # is one word.
        while line.endswith("\\") and at < len(rows):
            line = line[:-1] + rows[at]
            at += 1
        found = HEREDOC.search(line)
        if found:
            # The command written just before `<<` is the one that reads it.
            before, _ = strip_prefix(
                words(re.split(r"[;&|]", line[: found.start()])[-1])
            )
            reader = before[0].rsplit("/", 1)[-1] if before else ""
            # What reads the body decides what it is: a shell runs it, an
            # unquoted body is expanded so $(...) in it runs, and a quoted body
            # for anything else is only data (a script, a message, a fixture).
            if reader in SHELLS:
                mode = "shell"
            elif found.group(1):
                mode = "data"
            else:
                mode = "expanded"
            pending = (found.group(2), mode)
            line = HEREDOC.sub(" ", line)
        out.append(line)
    expand()
    return out


def standalone(line, at):
    """A brace that is a word of its own groups commands; `x.{a,b}` and `${v}` do not."""
    edge = " \t;&|(){}"
    return (at == 0 or line[at - 1] in edge) and (
        at + 1 == len(line) or line[at + 1] in edge
    )


def simple_commands(text):
    """Split on ; & | and newlines outside quotes, and add every nested command."""
    for line in lines(text):
        for found in SUBSTITUTION.findall(line):
            yield from simple_commands(found[0] or found[1])
        parts, current, quote = [], "", None
        for at, char in enumerate(line):
            if quote:
                current += char
                if char == quote:
                    quote = None
            elif char in "'\"":
                quote = char
                current += char
            elif char in ";&|()" or (char in "{}" and standalone(line, at)):
                parts.append(current)
                current = ""
            else:
                current += char
        parts.append(current)
        for part in parts:
            if part.strip():
                yield part.strip()


def words(command):
    try:
        return shlex.split(command, comments=False, posix=True)
    except ValueError:
        return command.split()


# Options of a wrapper that take the next word as their value; without this the
# value would be read as the program (`sudo -u root git ...`).
WRAPPER_VALUES = {
    "sudo": {"-u", "-g", "-h", "-p", "-C", "-D", "-R", "-T", "-U"},
    "env": {"-u", "-C", "-S"},
    "nice": {"-n"},
    "xargs": {"-I", "-n", "-P", "-L", "-s", "-d", "-E", "-a"},
    "time": {"-f", "-o"},
}
# Words that open a command in a compound statement.
KEYWORDS_BEFORE = {"if", "then", "do", "else", "elif", "while", "until", "!"}


def strip_prefix(tokens):
    """Drop keywords, assignments and wrappers before the program.

    Returns the tokens from the program on and the assignments that preceded it,
    since `GIT_CONFIG_KEY_0=core.hooksPath` redirects hooks without any option.
    """
    env = {}
    while tokens:
        head = tokens[0].lstrip("\\")
        name = head.rsplit("/", 1)[-1]
        assignment = re.match(r"^(\w+)=(.*)$", head)
        if assignment:
            env[assignment.group(1)] = assignment.group(2)
            tokens = tokens[1:]
        elif head in KEYWORDS_BEFORE:
            tokens = tokens[1:]
        elif name in WRAPPERS:
            tokens = tokens[1:]
            takes = WRAPPER_VALUES.get(name, set())
            while tokens and tokens[0].startswith("-"):
                tokens = tokens[2:] if tokens[0] in takes else tokens[1:]
        else:
            break
    return tokens, env


def git_parts(tokens):
    """The global options, subcommand and its arguments of a `git` command."""
    config, args = [], tokens[1:]
    while args and args[0].startswith("-"):
        option = args[0]
        if option in ("-c", "--config-env") and len(args) > 1:
            config.append(args[1])
            args = args[2:]
        elif option.startswith("--config-env="):
            config.append(option[len("--config-env=") :])
            args = args[1:]
        elif option.startswith("-c") and len(option) > 2:
            config.append(option[2:])
            args = args[1:]
        elif (
            option in ("-C", "--git-dir", "--work-tree", "--namespace", "--exec-path")
            and len(args) > 1
        ):
            args = args[2:]
        else:
            args = args[1:]
    return config, (args[0] if args else ""), args[1:]


def skips_hooks(verb, args):
    """Whether a history-moving verb is told to skip its hooks."""
    if verb not in NO_VERIFY_VERBS:
        return False
    commit = verb == "commit"
    skip = False
    for token in args:
        if skip:
            skip = False
        elif token.startswith("--"):
            if abbreviates(token, "--no-verify"):
                return True
            skip = commit and token in COMMIT_VALUES
        elif commit and token.startswith("-") and len(token) > 1:
            # `-anm msg` is -a -n -m: letters after one that takes a value are
            # that value, so stop there.
            for letter in token[1:]:
                if letter == "n":
                    return True
                if "-" + letter in COMMIT_VALUES:
                    skip = len(token) == 2
                    break
    return False


def config_reason(name):
    """Why setting this git configuration name would sidestep the hooks."""
    name = name.split("=", 1)[0].lower()
    if name == "core.hookspath":
        return HOOKS_PATH
    if name.startswith("alias."):
        return ALIAS
    return None


def nested_scripts(program, tokens):
    """The script text a shell or `eval` is given to run, if any."""
    if program == "eval":
        return [" ".join(tokens[1:])]
    if program in SHELLS:
        for flag, script in zip(tokens[1:], tokens[2:]):
            if flag.startswith("-") and flag.endswith("c"):
                return [script]
    return []


def git_reason(tokens):
    config, verb, args = git_parts(tokens)
    for entry in config:
        if config_reason(entry):
            return config_reason(entry)
    if skips_hooks(verb, args) or (
        verb not in NO_VERIFY_VERBS
        and any(abbreviates(token, "--no-verify") for token in args)
    ):
        # Any other verb: an alias already in the user's configuration can stand
        # for commit or push, and git accepts the flag wherever the verb does.
        # The verbs above read their own options, values included, in skips_hooks.
        return "skips the git hooks (--no-verify)"
    if verb in ("commit-tree", "fast-import", "filter-branch"):
        return "builds or rewrites commits without running the commit hooks"
    if verb == "update-ref" and "-d" not in args:
        return "moves a ref directly, past the commit and push hooks"
    if verb == "config" and not any(a in READS for a in args):
        # `git config NAME VALUE` sets; a name alone only reads.
        for i, arg in enumerate(args):
            if config_reason(arg) and (i + 1 < len(args) or "=" in arg):
                return config_reason(arg)
    return None


def reason(command):
    tokens, env = strip_prefix(words(command))
    if not tokens:
        return None
    program = tokens[0].lstrip("\\").rsplit("/", 1)[-1]
    if redirects_into_hooks(command):
        return HOOKS_CHANGE
    if redirects_from_credential(command) or reads_credential(program, tokens):
        return CREDENTIAL_READ
    for script in nested_scripts(program, tokens):
        for inner in simple_commands(script):
            found = reason(inner)
            if found:
                return found
    if program == "make" and "uninstall-hooks" in tokens:
        return "removes the installed git hooks"
    if program == "git":
        for key, value in env.items():
            if (
                re.fullmatch(r"GIT_CONFIG_KEY_\d+", key)
                and value.lower() == "core.hookspath"
            ):
                return HOOKS_PATH
            if key == "GIT_CONFIG_PARAMETERS" and "core.hookspath" in value.lower():
                return HOOKS_PATH
        return git_reason(tokens)
    if program == "sed":
        # Reading a hook with sed is fine; only -i edits it.
        in_place = any(t.startswith(("-i", "--in-place")) for t in tokens[1:])
        if in_place and any(HOOK_FILE.search(t) for t in tokens[1:]):
            return HOOKS_CHANGE
    if program == "dd" and any(
        HOOK_FILE.search(t[3:]) for t in tokens[1:] if t.startswith("of=")
    ):
        return HOOKS_CHANGE
    if program in WRITERS:
        operands = [t for t in tokens[1:] if not t.startswith("-")]
        # Copying, linking and truncating write the last operand; the rest
        # (rm, mv, chmod) touch every one.
        targets = operands[-1:] if program in DESTINATION_LAST else operands
        if any(HOOK_FILE.search(t) for t in targets):
            return HOOKS_CHANGE
    return None


def reads_credential(program, tokens):
    """Whether a reader is pointed at a credential file, named or as `x=path`.

    A program that loads the file for its own use (`source`, `node script file`)
    is not in READERS: what is refused is putting the contents in the transcript.
    It reads the operands as written: a relative path after `cd config`, or
    output substituted in by `$(...)`, is not followed.
    """
    if program not in READERS:
        return False
    if program in SEARCHERS and names_only(program, tokens[1:]):
        return False
    operands, targeted = [], False
    args = iter(tokens[1:])
    for token in args:
        if program in TO_LAST and token in ("-t", "--target-directory"):
            targeted = next(args, None) is not None
        elif program in TO_LAST and token.startswith("--target-directory="):
            targeted = True
        elif token.startswith("--") and "=" in token:
            operands.append(token.split("=", 1)[1])  # `grep --file=config/...`
        elif not token.startswith("-"):
            operands.append(token)
    if program == "find" and not {"-exec", "-execdir", "-ok", "-okdir"} & set(tokens):
        return False  # without an action find prints names, not contents
    if program in TO_LAST and not targeted:
        operands = operands[:-1]
    return any(reaches_credentials(t.split("=", 1)[-1]) for t in operands)


def reaches_credentials(path):
    """Whether a path names, globs or is the directory of the credential files."""
    path = posixpath.normpath(path)
    if CREDENTIAL_FILE.search(path) or CREDENTIAL_REACH.search(path):
        return True
    # The directory itself is only this repository's `config`, not `.git/config`
    # or `src/config`: a recursive reader on it prints every file inside.
    if path == "config":
        return True
    try:
        here = os.environ.get("CLAUDE_PROJECT_DIR") or os.getcwd()
    except OSError:  # the working directory was deleted: fail to the lexical rule
        return False
    return path == posixpath.join(here, "config")


def names_only(program, args):
    """Whether a search prints file names or counts, never the matching lines.

    `-L` lists files without a match for grep, but is `--follow` for rg and ag,
    which still print matching lines. Only options count: the scan stops at `--`
    and skips the pattern an option such as `-e` takes, so `grep -e -c file` is
    a search for "-c".
    """
    letters = set("lLqc") if program in ("grep", "egrep", "fgrep") else set("lqc")
    args = iter(args)
    for arg in args:
        if arg == "--":
            return False
        if arg in VALUE_OPTIONS:
            next(args, None)
        elif arg in NAME_FLAGS or (
            re.fullmatch(r"-[A-Za-z]+", arg) and set(arg[1:]) & letters
        ):
            return True
    return False


def redirect_targets(command, operator):
    """The paths a command redirects to (`>`, `>>`) or from (`<`)."""
    pattern = r""">>?\s*(?:"([^"]*)"|'([^']*)'|(\S+))"""
    if operator == "<":
        pattern = r"""(?<!<)<(?![<(&])\s*(?:"([^"]*)"|'([^']*)'|(\S+))"""
    for found in re.finditer(pattern, command):
        yield found.group(1) or found.group(2) or found.group(3) or ""


def redirects_into_hooks(command):
    """`> path` or `>> path` where the path is a hook or the script behind one."""
    return any(HOOK_FILE.search(t) for t in redirect_targets(command, ">"))


def redirects_from_credential(command):
    """`< path` taking a credential file as input."""
    return "config" in command and any(
        reaches_credentials(t) for t in redirect_targets(command, "<")
    )


def main():
    try:
        call = json.load(sys.stdin)
    except ValueError:
        return 0
    if call.get("tool_name") != "Bash":
        return 0
    text = (call.get("tool_input") or {}).get("command", "")
    for command in simple_commands(text):
        why = reason(command)
        if why:
            hint = (
                "Name the variable you need to see set or unset, not its value, "
                "or ask the user to run the command themselves."
                if why == CREDENTIAL_READ
                else "The hooks generate the Change-Id and check the interactive "
                "review; CI checks both again, so skipping them here only moves "
                "the failure. Fix what the hook reports, or ask the user to run "
                "the command themselves if it really is needed."
            )
            print(f"Blocked: this command {why}.\n{hint}", file=sys.stderr)
            return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
