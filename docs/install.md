# Installing a prebuilt binary

Where the published binaries come from, what they do and do not carry, and what
each platform asks of you before it will run one. Building from a checkout is in
[README.md](../README.md#run) and needs none of this.

Every successful build of `main` replaces the `latest` release under
<https://github.com/sysprog21/codetrial/releases>, unless a newer commit landed
while it was running. The binaries go to a draft first, and their sizes are
checked against the files the build produced before that draft takes the name,
so a truncated upload never reaches the download page. Two things that tag will
not give you: replacing a release is not atomic, so each publish has a short
window where `latest` resolves to nothing, and the tag moves, so a link does not
keep serving the bytes it served last week. Pin a commit and keep your own
checksum if you need the same binary twice.

Every binary knows the commit it was built from. `./codetrial --version` prints
it without reading any configuration, and `web` mode prints the same line first
on startup. Quote it when reporting a problem: the package version does not
change from one build of `main` to the next, so the commit is the only thing
that says which build you have.

Nothing is required at runtime beyond the binary itself: the browser
application, its vendored assets, and the WASM are compiled in, so there is no
Node.js, no `node_modules`, and no `web/` directory to unpack alongside it. The
avatar model is not in there either: the browser downloads it once from a pinned
upstream URL, checks it against a pinned SHA-256, and keeps it in its own cache.
Without that reachable, the interview falls back to Jim's voice-only panel and
nothing else changes.

On a network that reaches the internet only through a proxy, set `HTTPS_PROXY`
to an `http://` proxy URL. The binary's HTTP requests, the Gemini Live socket
and LiveKit signaling go through it; `NO_PROXY` exempts hosts from all but
LiveKit signaling, which LiveKit's transport proxies regardless. LiveKit's audio
and video travel over WebRTC, which does not use the proxy, and the browser
fetches the avatar model through its own proxy settings, not this one.
Certificates are still checked against Mozilla's roots compiled into the binary,
so a TLS-inspecting proxy, whose certificate authority is installed only on the
machine, is refused.

```bash
# Linux
tar -xzf codetrial-x86_64-unknown-linux-gnu.tar.gz
mv codetrial-x86_64-unknown-linux-gnu codetrial

# macOS
unzip codetrial-aarch64-apple-darwin.zip
xattr -d com.apple.quarantine codetrial-aarch64-apple-darwin
mv codetrial-aarch64-apple-darwin codetrial
```

Run `./codetrial` with no config file nearby, and `web` mode serves a Setup
page at <http://127.0.0.1:3000> instead of refusing to start, printing that
address for you to open. Loopback is the only place it will serve:
`--web-addr 0.0.0.0:3000` or `CODETRIAL_WEB_ADDR` pointing anywhere else is
refused, because the page takes credentials over plain HTTP from anyone who can
reach it.

Enter your LiveKit keys there, and your Google key if you want this process to
host interviewers (see below). Saving writes them **in plain text** to
`config/codetrial.env.local` beside the executable, creating that directory: the
same file you would otherwise write yourself.

```bash
mkdir -p config
cat >config/codetrial.env.local <<'EOF'
LIVEKIT_URL=wss://YOUR-PROJECT.livekit.cloud
LIVEKIT_API_KEY=...
LIVEKIT_API_SECRET=...
GOOGLE_API_KEY=...
EOF
./codetrial web   # http://127.0.0.1:3000
```

That `config/` is your data: the keys above and, once the server has run,
`codetrial.db` with your accounts and reports. To take a newer build, replace
the executable in place and leave `config/` alone. Unpacking into a fresh
directory starts over, and deleting the directory removes everything this
program kept.

The binary does not say when `latest` has moved on unless you ask it to. On
macOS, `CODETRIAL_UPDATE_CHECK=true` in that config file has the lobby check
when it loads, which includes coming back from an interview, and again on every
hour it stays open. The server asks GitHub's API which commit `latest` names,
at most once every ten minutes however many tabs are open, and when that is not
the commit `./codetrial --version` prints, the lobby shows a notice linking to
the release. Dismissing it hides it until a later release. A check that fails,
offline or while `latest` is being republished, shows nothing. It is off by
default because it is the one request the server makes to GitHub on its own:
GitHub sign-in reaches GitHub when a candidate signs in, on their account, and
the avatar model is fetched by the interview page, while this check is the
server asking whenever a lobby is open, with no account. It does nothing on
Linux, or in a build from source, whose commit the release will never name.
The desktop app never checks either: it is published as `desktop-latest`,
which this does not read, so it starts its server with `--no-update-check`,
which turns the setting off over the config file.

`GOOGLE_API_KEY` is the optional one, and it decides which of two things this
process is. With it, the server hosts interviewers itself. Without it, it serves
the web side only and every room waits for an agent to join from elsewhere, and
it says so on startup. There is no matching line for the hosting case, so a
silent start is the one that hosts interviewers. `--config PATH` names the file
if you would rather keep it somewhere else; see
[Configuration](../README.md#configuration) for the rest.

Platform notes:

- Linux binaries are built against glibc 2.31 and the libstdc++ that ships
  beside it, the pair Ubuntu 20.04 and Debian 11 carry, so they run there and
  on anything newer. The release build checks that floor and fails rather than
  publishing a binary that asks for more, because a system whose glibc is
  older refuses one in the loader, before `main`, naming a symbol version it
  does not have. Older distributions build from source.
- macOS binaries carry only the ad-hoc signature the linker applies, which is
  what lets an arm64 binary run at all. They are not Developer ID signed and not
  notarized, because that needs a paid Apple Developer Program membership this
  project does not have. A downloaded `.zip` carries the quarantine attribute
  and Gatekeeper refuses it, so clear the attribute as above or build from
  source.
- Windows binaries are unsigned. SmartScreen warns on first run. Put the exe in
  a folder of its own before double-clicking it: with no config file nearby it
  opens the Setup page above, and everything it writes lands in that folder, the
  `config/` that page fills in, and, if the start fails, a
  `codetrial-error.log` beside the executable. That log names the build it
  came from before the reason, and is the only place the reason appears when
  there is no terminal to read one from.
- Only `x86_64` Linux, `arm64` macOS, and `x86_64` Windows are published. Build
  from source for anything else.
