"""The agent guard blocks hook bypasses and nothing an honest commit needs."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

GUARD = Path(__file__).resolve().parents[1] / "scripts/agent-guard.py"

BLOCKED = [
    # The account database holds session tokens and candidates' reports.
    "sqlite3 config/codetrial.db .dump",
    "cat config/codetrial.db-wal",
    # Found by the pull request review.
    "git commit \\\n  --no-verify -m x",
    "git commit --no-\\\nverify -m x",
    "(git commit --no-verify -m x)",
    "{ git commit --no-verify -m x; }",
    "if true; then git commit --no-verify -m x; fi",
    "while true; do git push --no-verify origin HEAD; done",
    "! git commit --no-verify -m x",
    "GIT_CONFIG_COUNT=1 GIT_CONFIG_KEY_0=core.hooksPath GIT_CONFIG_VALUE_0=/dev/null git commit -m x",
    "GIT_CONFIG_PARAMETERS=\"'core.hookspath=/dev/null'\" git commit -m x",
    "git --config-env=core.hooksPath=HP commit -m x",
    "git --config-env core.hooksPath=HP commit -m x",
    "sudo -u root git commit --no-verify -m x",
    "nice -n 5 git commit --no-verify -m x",
    "env -u HOME git commit --no-verify -m x",
    "git c --no-verify -m x",
    # A quoted heredoc body ending in a backslash does not swallow its delimiter.
    "cat <<'EOF'\nfoo\\\nEOF\ngit commit --no-verify -m x",
    "time -p git commit --no-verify -m x",
    # A command substitution in an unquoted heredoc body runs, on several lines too.
    "cat <<EOF\n$(\ngit commit --no-verify -m x\n)\nEOF",
    "cat <<EOF\nsay `git commit --no-verify -m x`\nEOF",
    "git ci --no-verify",
    "dd if=/dev/null of=scripts/git-pre-push.sh",
    'echo x > "scripts/git-pre-push.sh"',
    "echo x >> '.git/hooks/pre-push'",
    "sed -i 's/exit 1/exit 0/' scripts/git-pre-push.sh",
    "sed --in-place -e 's/a/b/' .git/hooks/pre-push",
    # Spellings and shapes that review found getting past a text search.
    'git commit "--no-verify" -m x',
    "git commit '--no-verify' -m x",
    "git commit --no-veri -m x",
    "git commit --no-verif -m x",
    "git push --no-ve origin HEAD",
    "git -c alias.c=commit c -n -m x",
    "git config alias.c commit",
    "git -C . update-ref refs/heads/topic HEAD",
    "git -c core.hookspath=/dev/null commit -m x",
    "git -c CORE.HOOKSPATH=/dev/null commit -m x",
    'git -c core.hooksPath=/dev/null commit -m "--unset"',
    "git config core.hookspath /dev/null",
    "cat <<EOF; git commit --no-verify -m x\nhello\nEOF",
    "cat <<EOF\n$(git commit --no-verify -m x)\nEOF",
    "bash <<EOF\ngit commit --no-verify -m x\nEOF",
    "cd x && bash <<'EOF'\ngit commit --no-verify -m x\nEOF",
    "echo $(git commit -n -m x)",
    'eval "git commit --no-verify -m x"',
    'bash -c "git commit --no-verify -m x"',
    "sudo git commit --no-verify -m x",
    "command git commit --no-verify -m x",
    "\\git commit --no-verify -m x",
    "echo x | xargs git commit --no-verify -m",
    "FOO=1 git commit --no-verify -m x",
    "make uninstall-hooks",
    "cp /dev/null scripts/git-pre-push.sh",
    "cat /dev/null > scripts/git-pre-push.sh",
    "sh -c 'echo > .git/hooks/pre-push'",
    "git commit --no-verify -m 'x'",
    "git commit -n -m 'x'",
    "git commit -anm 'x'",
    "git -C repo commit --no-verify",
    "git push --no-verify origin HEAD",
    "git merge --no-verify topic",
    "git rebase --continue --no-verify",
    "git cherry-pick -x abc --no-verify",
    "cd x && git commit --no-verify -m y",
    "sh -c 'git commit --no-verify -m y'",
    "git config core.hooksPath /dev/null",
    "git -c core.hooksPath=/dev/null commit -m y",
    "git config --global core.hooksPath /tmp/none",
    "git commit-tree HEAD^{tree} -p HEAD -m y",
    "git update-ref refs/heads/topic abc123",
    "git fast-import < dump",
    "git filter-branch --env-filter true",
    "rm .git/hooks/pre-push",
    "chmod -x .git/hooks/commit-msg",
    "echo exit 0 > .git/hooks/pre-push",
    "make uninstall-hooks && rm -rf .git/hooks",
    "sed -i 's/exit 1/exit 0/' scripts/git-pre-push.sh",
]

BLOCKED += [
    "cat config/codetrial.env.{local,prod}",
    "find config -exec cat {} +",
    "grep --file=config/codetrial.env.local x",
    "grep -e -c config/codetrial.env.local",
    "cat < config/./codetrial.env.local",
    "cat 0<config/codetrial.env.local",
    "grep -n -- -l config/codetrial.env.local",
    "rg -L KEY config",
    "ag -L KEY config",
    "sh -c 'cat < config/codetrial.env.local'",
    "cat config//codetrial.env.local",
    "cat config/./codetrial.env.local",
    "cat config/x/../codetrial.env.local",
    "dd if=config/codetrial.env.local",
    "cp --target-directory=/tmp config/codetrial.env.local",
    "cp -t /tmp config/codetrial.env.local",
    "grep -rn KEY config",
    "cat config/*.local",
    "rg KEY config/",
    "tar cf - config/codetrial.env.local",
    "cat config/codetrial.env.local",
    "grep -n KEY config/codetrial.env.bochengC",
    "head -3 ./config/codetrial.env.prod",
    "cp config/codetrial.env.local /tmp/x",
    "sh -c 'cat config/codetrial.env.local'",
    'while read -r l; do echo "$l"; done < config/codetrial.env.local',
    "sed -n 1,5p /home/u/repo/config/codetrial.env.local",
]

ALLOWED = [
    "find config -name '*.example'",
    "echo ${HOME} ${USER}",
    "grep -rL KEY config",
    "cat .git/config",
    "grep url .git/config",
    'grep -rn "src/config" src',
    "cat .cargo/config.toml",
    "cp -t config config/codetrial.env.example",
    "rg --files config",
    "grep -rl KEY config",
    "grep -c KEY config/codetrial.env.local",
    "cat config/*.example",
    "grep -n KEY config/codetrial.env.example",
    "ls config/",
    "cat config/codetrial.env.example",
    "cp config/codetrial.env.example config/codetrial.env.local",
    "set -a; . config/codetrial.env.local; set +a",
    "node scripts/livekit-room-service.cjs config/codetrial.env.local",
    "ls config",
    "git check-ignore config/codetrial.env.local",
    "python3 scripts/check-secrets.py",
    "grep -n LIVEKIT docs/livekit-connection-troubleshooting.md",
    "if git diff --quiet; then echo clean; fi",
    "(cd scripts && ls)",
    "{ echo a; echo b; }",
    "git log --format=%(refname) -3",
    "nice -n 5 make hooks",
    "sudo -u root ls",
    "env -u HOME ls",
    "dd if=a of=/tmp/b",
    'echo x > "/tmp/out.txt"',
    "GIT_AUTHOR_NAME=x git commit -m fix",
    "sed -n 196,330p scripts/git-pre-push.sh",
    "sed -e 's/a/b/' scripts/git-pre-push.sh",
    "git config --get core.hooksPath",
    "git config --get-all core.hookspath",
    "git config core.hooksPath",
    'git commit -m "Document the -n flag"',
    "git commit -m 'explain --no-verify to readers'",
    "git commit -m 'Handle -n' -m 'and --no-verify in docs'",
    "git push origin topic -n",
    "git log --grep=no-verify",
    # The word after -m is the message, whatever it looks like.
    "git commit -m --no-verify",
    "cat <<EOF\nhello $(date)\nEOF",
    "cat <<'EOF'\n$(\ngit commit --no-verify -m x\n)\nEOF",
    "git diff scripts/git-pre-push.sh",
    "cp scripts/git-pre-push.sh /tmp/copy.sh",
    "cat scripts/git-pre-push.sh",
    'echo "git commit --no-verify" | wc -c',
    "sh -c 'git commit -m fix'",
    # A quoted heredoc to a non-shell is data: a fixture may name the flag.
    "cd x && python3 - <<'EOF'\nprint('git commit --no-verify')\nEOF",
    "cat <<'EOF'\ngit commit --no-verify\nEOF",
    "git commit -m 'fix the parser'",
    "git commit -am 'update'",
    "git commit -m 'Handle the n case' -m 'body'",
    "git commit --amend --no-edit",
    "git push origin HEAD",
    "git push -n origin HEAD",
    "git push --dry-run origin HEAD",
    "git merge --no-ff topic",
    "git rebase -i upstream/main",
    "git config --unset core.hooksPath",
    "git config user.name x",
    "git update-ref -d refs/heads/old",
    "git log --oneline",
    "make hooks",
    "python3 scripts/hashchain.py rechain",
    "cat .git/hooks/pre-push",
    "ls .git/hooks",
    # A heredoc body is data: documentation may mention the flag.
    "python3 - <<'EOF'\nprint('git commit --no-verify')\nEOF",
    "grep -rn no-verify docs",
]


def guard(tool, tool_input, project=None):
    """The guard's answer to one tool call, run as Claude Code runs it, from
    `project` when one is given."""
    env = dict(os.environ)
    if project is not None:
        env["CLAUDE_PROJECT_DIR"] = str(project)
    return subprocess.run(
        [sys.executable, str(GUARD)],
        input=json.dumps({"tool_name": tool, "tool_input": tool_input}),
        text=True,
        capture_output=True,
        cwd=project,
        env=env,
    )


def run(command, tool="Bash"):
    return guard(tool, {"command": command})


class AgentGuardTests(unittest.TestCase):
    def test_hook_bypasses_are_blocked_with_a_reason(self):
        for command in BLOCKED:
            with self.subTest(command=command):
                result = run(command)
                self.assertEqual(result.returncode, 2, result.stderr)
                self.assertIn("Blocked", result.stderr)

    def test_ordinary_git_use_is_allowed(self):
        for command in ALLOWED:
            with self.subTest(command=command):
                result = run(command)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_other_tools_and_unreadable_input_are_left_alone(self):
        self.assertEqual(run("git commit --no-verify", tool="Read").returncode, 0)
        result = subprocess.run(
            [sys.executable, str(GUARD)], input="not json", text=True
        )
        self.assertEqual(result.returncode, 0)

    def test_settings_register_the_guard_for_every_tool_it_judges(self):
        settings = json.loads((GUARD.parents[1] / ".claude/settings.json").read_text())
        entry = settings["hooks"]["PreToolUse"][0]
        for tool in (
            "Bash",
            "Read",
            "Grep",
            "Edit",
            "MultiEdit",
            "Write",
            "NotebookEdit",
        ):
            with self.subTest(tool=tool):
                self.assertRegex(tool, f"^(?:{entry['matcher']})$")
        self.assertIn("scripts/agent-guard.py", entry["hooks"][0]["command"])

    def test_a_tool_that_opens_a_path_is_judged_like_its_shell_reader(self):
        root = str(GUARD.parents[1])
        for tool, tool_input, status in (
            ("Read", {"file_path": f"{root}/config/codetrial.env.local"}, 2),
            ("Read", {"file_path": "config/codetrial.env.alice"}, 2),
            ("Read", {"file_path": f"{root}/config/codetrial.env.example"}, 0),
            ("Read", {"file_path": f"{root}/config/codetrial.db"}, 2),
            ("Read", {"file_path": "config/codetrial.db-wal"}, 2),
            ("Read", {"file_path": f"{root}/docs/codetrial.db.md"}, 0),
            ("Read", {"file_path": f"{root}/src/config.rs"}, 0),
            (
                "Grep",
                {"pattern": "KEY", "path": f"{root}/config", "output_mode": "content"},
                2,
            ),
            # Names and counts, Grep's default, show no line of the file.
            ("Grep", {"pattern": "KEY", "path": f"{root}/config"}, 0),
            ("Grep", {"pattern": "KEY", "path": "config", "output_mode": "count"}, 0),
            ("Edit", {"file_path": f"{root}/.git/hooks/pre-commit"}, 2),
            ("Write", {"file_path": ".git/hooks/pre-push"}, 2),
            ("MultiEdit", {"file_path": f"{root}/.git/hooks/commit-msg"}, 2),
            ("NotebookEdit", {"notebook_path": f"{root}/.git/hooks/pre-push"}, 2),
            ("Edit", {"file_path": f"{root}/scripts/../.git/hooks/pre-commit"}, 2),
            # The repository's git config can repoint core.hooksPath.
            ("Edit", {"file_path": f"{root}/.git/config"}, 2),
            ("Write", {"file_path": f"{root}/.git/config.worktree"}, 2),
            ("Edit", {"file_path": f"{root}/.git/worktrees/w/config.worktree"}, 2),
            ("Read", {"file_path": f"{root}/.git/config"}, 0),
            # The tracked script behind a hook is reviewed and tested as code.
            ("Edit", {"file_path": f"{root}/scripts/git-pre-commit.sh"}, 0),
            ("Write", {"file_path": f"{root}/config/codetrial.env.local"}, 0),
        ):
            with self.subTest(tool=tool, tool_input=tool_input):
                result = guard(tool, tool_input, root)
                self.assertEqual(result.returncode, status, result.stderr)
                if status:
                    self.assertIn("Blocked", result.stderr)


class FileToolPathTests(unittest.TestCase):
    """A scratch project holding a credential file, for what only a real file
    system shows: a glob ripgrep matches against it, and a symlink to it."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "config").mkdir()
        (self.root / "config/codetrial.env.local").write_text("KEY=secret\n")
        (self.root / "config/codetrial.env.example").write_text("KEY=\n")

    def guard(self, tool, tool_input):
        return guard(tool, tool_input, self.root).returncode

    def test_a_grep_glob_that_names_a_credential_file_is_refused(self):
        for glob in ("config/*.env.local", "*.env.*", "*.local", "{*.local,*.md}"):
            with self.subTest(glob=glob):
                self.assertEqual(
                    self.guard(
                        "Grep",
                        {"pattern": "KEY", "glob": glob, "output_mode": "content"},
                    ),
                    2,
                )
                self.assertEqual(
                    self.guard("Grep", {"pattern": "KEY", "glob": glob}), 0
                )
        for glob in ("*.example", "*.rs", "!*.local"):
            with self.subTest(glob=glob):
                self.assertEqual(
                    self.guard(
                        "Grep",
                        {"pattern": "KEY", "glob": glob, "output_mode": "content"},
                    ),
                    0,
                )

    def test_a_grep_glob_is_scoped_to_the_search_path(self):
        (self.root / "src").mkdir()
        (self.root / "source").symlink_to(self.root / "src")
        for path, expected in (
            ("src", 0),
            (str(self.root / "src"), 0),
            ("source", 0),
            ("src/..", 2),
        ):
            with self.subTest(path=path):
                self.assertEqual(
                    self.guard(
                        "Grep",
                        {
                            "pattern": "KEY",
                            "path": path,
                            "glob": "*",
                            "output_mode": "content",
                        },
                    ),
                    expected,
                )

    def test_slashed_globs_match_from_the_search_or_project_directory(self):
        for glob, expected in (
            (f"/{self.root.name}/config/*.local", 2),
            ("config/*.local", 2),
            ("**/config/**/codetrial.env.local", 2),
            ("/codetrial.env.local", 0),
            ("/*/*/*.local", 2),
            ("/*/*/*/*.local", 0),
            ("config/codetrial.env.local/**", 0),
        ):
            with self.subTest(glob=glob):
                self.assertEqual(
                    self.guard(
                        "Grep",
                        {
                            "pattern": "KEY",
                            "path": "..",
                            "glob": glob,
                            "output_mode": "content",
                        },
                    ),
                    expected,
                )

    def test_a_symlink_to_a_credential_file_is_judged_by_its_target(self):
        (self.root / "notes.txt").symlink_to(self.root / "config/codetrial.env.local")
        self.assertEqual(
            self.guard("Read", {"file_path": str(self.root / "notes.txt")}), 2
        )
        self.assertEqual(self.guard("Read", {"file_path": "notes.txt"}), 2)
        (self.root / "plain.txt").write_text("x\n")
        self.assertEqual(self.guard("Read", {"file_path": "plain.txt"}), 0)

    def test_the_account_database_is_guarded_wherever_it_is_kept(self):
        # --data-dir keeps it under its own name elsewhere; CODETRIAL_DB_PATH
        # names any file, from the environment or a config file.
        elsewhere = self.root / "data"
        elsewhere.mkdir()
        (elsewhere / "codetrial.db").write_text("")
        self.assertEqual(
            self.guard("Read", {"file_path": str(elsewhere / "codetrial.db")}), 2
        )
        (self.root / "config/codetrial.env.local").write_text(
            "KEY=secret\nexport CODETRIAL_DB_PATH='accounts/store.sqlite'\n"
        )
        self.assertEqual(self.guard("Read", {"file_path": "accounts/store.sqlite"}), 2)
        self.assertEqual(
            self.guard("Read", {"file_path": "accounts/store.sqlite-wal"}), 2
        )
        self.assertEqual(self.guard("Read", {"file_path": "accounts/other.sqlite"}), 0)
        refused = subprocess.run(
            [sys.executable, str(GUARD)],
            input=json.dumps(
                {"tool_name": "Read", "tool_input": {"file_path": "/tmp/held.db"}}
            ),
            text=True,
            capture_output=True,
            cwd=self.root,
            env={
                **os.environ,
                "CLAUDE_PROJECT_DIR": str(self.root),
                "CODETRIAL_DB_PATH": "/tmp/held.db",
            },
        )
        self.assertEqual(refused.returncode, 2)

    def test_a_grep_glob_reaching_the_account_database_is_refused(self):
        # The glob is judged against what the predicate above guards, not only
        # against config/: the database under its own name anywhere, and the
        # file CODETRIAL_DB_PATH names, from a search root config/ is not under.
        (self.root / "data").mkdir()
        (self.root / "config/codetrial.env.local").write_text(
            "export CODETRIAL_DB_PATH='accounts/store.sqlite'\n"
        )
        for path, glob, expected in (
            (".", "**/codetrial.db", 2),
            ("data", "codetrial.db-wal", 2),
            (".", "accounts/*.sqlite", 2),
            ("accounts", "*.sqlite-shm", 2),
            ("data", "*.md", 0),
            ("accounts", "other/*.sqlite", 0),
        ):
            with self.subTest(path=path, glob=glob):
                self.assertEqual(
                    self.guard(
                        "Grep",
                        {
                            "pattern": "KEY",
                            "path": path,
                            "glob": glob,
                            "output_mode": "content",
                        },
                    ),
                    expected,
                )

    def test_a_glob_is_read_in_ripgreps_syntax(self):
        # ripgrep escapes with a backslash and negates a class with `^`; each
        # of these reaches config/codetrial.env.local through it.
        for glob in (
            "config/[^x]odetrial.env.local",
            "config/\\codetrial.env.local",
            "config/codetrial.env.loca[^x]",
            "\\config/*.local",
        ):
            with self.subTest(glob=glob):
                self.assertEqual(
                    self.guard(
                        "Grep",
                        {"pattern": "KEY", "glob": glob, "output_mode": "content"},
                    ),
                    2,
                )
        # An escaped star is a star: it names no credential file.
        self.assertEqual(
            self.guard(
                "Grep",
                {
                    "pattern": "KEY",
                    "glob": "codetrial.env.\\*",
                    "output_mode": "content",
                },
            ),
            0,
        )

    def test_a_braced_glob_is_expanded_before_it_is_judged(self):
        for glob in (
            "config/*.env.{local,x}",
            "config/{a,*.env.{loc,local}}",
            "*.{md,local}",
        ):
            with self.subTest(glob=glob):
                self.assertEqual(
                    self.guard(
                        "Grep",
                        {"pattern": "KEY", "glob": glob, "output_mode": "content"},
                    ),
                    2,
                )
        self.assertEqual(
            self.guard(
                "Grep",
                {
                    "pattern": "KEY",
                    "glob": "config/*.{rs,md}",
                    "output_mode": "content",
                },
            ),
            0,
        )

    def test_an_unexpected_input_shape_is_left_alone(self):
        self.assertEqual(self.guard("Read", {"file_path": 7}), 0)
        self.assertEqual(self.guard("Grep", {"pattern": "x", "glob": ["*.local"]}), 0)


if __name__ == "__main__":
    unittest.main()
