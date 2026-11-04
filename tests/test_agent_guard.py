"""The agent guard blocks hook bypasses and nothing an honest commit needs."""

import json
import subprocess
import sys
import unittest
from pathlib import Path

GUARD = Path(__file__).resolve().parents[1] / "scripts/agent-guard.py"

BLOCKED = [
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


def run(command, tool="Bash"):
    return subprocess.run(
        [sys.executable, str(GUARD)],
        input=json.dumps({"tool_name": tool, "tool_input": {"command": command}}),
        text=True,
        capture_output=True,
    )


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

    def test_settings_register_the_guard_for_the_bash_tool(self):
        settings = json.loads((GUARD.parents[1] / ".claude/settings.json").read_text())
        entry = settings["hooks"]["PreToolUse"][0]
        self.assertEqual(entry["matcher"], "Bash")
        self.assertIn("scripts/agent-guard.py", entry["hooks"][0]["command"])


if __name__ == "__main__":
    unittest.main()
