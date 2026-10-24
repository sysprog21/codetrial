# The desktop app

CodeTrial also ships as a desktop app for macOS, Windows and Linux: one window,
a server it starts and stops itself, and nothing to open in a browser by hand.
It is the same server as `codetrial web`, so the interview, the reports and the
rules in the other documents here are unchanged. What follows is what the app
adds around that server, where it keeps things, and what it has not been
qualified for yet.

The browser deployment is not going anywhere. Everything below is an
alternative way to run the same program on one person's machine.

## What it is

`desktop/` is a [Tauri 2](https://v2.tauri.app/) app. It does not link the
server in: it runs the `codetrial` executable that ships beside it as a child
process, and its window shows the page that child serves on
`http://127.0.0.1:3101`.

A child process rather than a library call, because the server's startup lives
in `src/main.rs` (config search, the Setup handover, every refusal it makes
before serving) and moving that into the library would have been a rewrite of
the file the CLI's behavior is pinned to. As a child it is the binary the CLI
tests already cover, started with four things a launcher needs that the CLI
did not have. Each is decided in the server, so the app keeps no copy of the
server's rules:

- `--data-dir DIR`: the config file, the account database, the session key,
  the error log and any `web/` override all live in `DIR`, unless the config
  file there names another place with `CODETRIAL_DB_PATH` or
  `CODETRIAL_WEB_DIR` (a relative one resolves against the working
  directory, which the app sets to `DIR`). The working directory and the
  executable's folder are never searched or written, because a launcher
  chooses neither, and no configuration is read from the environment, because
  a shell that exported `CODETRIAL_DB_PATH` or `NODE_ENV` for a CLI deployment
  would otherwise move this install's reports depending on how it was started.
  The one exception is a first launch: when `DIR` has no config yet and
  `LIVEKIT_URL`, `LIVEKIT_API_KEY` and `LIVEKIT_API_SECRET` are exported, those
  (and `GOOGLE_API_KEY` or `GOOGLE_API_KEYS`, if set) are written to `DIR`'s config as Setup would
  write them, checked the way Setup checks them but not probed, and Setup is
  skipped. Nothing else from the environment goes in, and later launches read
  only the file.
- `--import-config PATH`: a config Setup offers to use. The page names `PATH`
  and the scheme and host of its LiveKit URL, and says whether it holds a
  Google key, but never shows a key, the rest of the URL or a line it could not
  read: any account on the machine can reach the port while Setup serves.
  "Use these keys" has the server read `PATH` and save its Setup keys, checked
  as typed ones are, and nothing else in it, so a CLI deployment's database
  path or room prefix stays the CLI's. A `PATH` that cannot be used is named
  with the reason instead. Never used unasked: a file found near where the app
  started could be any project's, and its LiveKit project is where interviews
  would go. The app passes the CLI config nearest the folder it was started
  in: `config/codetrial.env.local` or `codetrial.env.local` there, or in a
  folder above it, so a development run from a checkout's `desktop/` finds the
  checkout's `config/`.
- `--allowed-hosts LIST` (or `CODETRIAL_ALLOWED_HOSTS`): the `Host` values the
  server answers. A page on another origin can repoint its own name at
  127.0.0.1 and read a loopback server as same origin; the name it sends is
  the one thing that still gives it away. The app passes `127.0.0.1:3101` and
  nothing else.
- `CODETRIAL_EXIT_WITH_STDIN`: the server exits when its standard input
  closes. The app holds the other end of that pipe, so when the app goes, by
  quitting or by crashing, the server goes too and does not keep the port.
  This one stays in the environment even for `--data-dir`, because it is not
  configuration but a description of the pipe the launcher handed over.

All four are ordinary server options, and a CLI install can use them the same
way. `codetrial --help` lists the three flags, and the README's configuration
table lists `CODETRIAL_ALLOWED_HOSTS`.

## Where it keeps things

One folder per user, chosen by the operating system:

| Platform | Folder                                                |
| -------- | ----------------------------------------------------- |
| macOS    | `~/Library/Application Support/org.sysprog21.codetrial` |
| Windows  | `%APPDATA%\org.sysprog21.codetrial`                   |
| Linux    | `$XDG_DATA_HOME/org.sysprog21.codetrial`, usually `~/.local/share/org.sysprog21.codetrial` |

In it:

- `codetrial.env.local`: the keys the Setup page takes, in plain text, the
  same file the CLI writes. Edit it to change a key or add an option. The first
  launch writes it from exported LiveKit keys without asking (see `--data-dir`
  above); otherwise Setup asks, and offers to use a CLI config found near where
  the app was started (see `--import-config`). An app opened from the Dock,
  Finder or the Start menu sees no shell exports and starts where no CLI config
  is, so Setup's "Import from a config file" button is the way to bring one in:
  it fills the form from a file you choose. Either way it asks once: later
  launches read this file.
- `codetrial.db`: accounts and saved reports.
- `session-secret`: the key session cookies are signed with, which the server
  makes on its first `--data-dir` launch when the config file sets no
  `SESSION_SECRET`, owner-only on macOS and Linux. The CLI's built-in default
  is a published string, and any program on the machine can reach the port, so
  a data folder never uses it. A damaged file stops the server rather than
  being replaced, because replacing it signs everyone out and hides what
  damaged it.
- `codetrial-server.log`: everything the server printed on its last launch.
- `codetrial-error.log`: why the last launch failed, when it did.

What the page itself stores (practice history among it) is the web view's,
keyed by the origin `http://127.0.0.1:3101`. That is why the port is fixed: a
port chosen per launch would be a new origin, and an empty history, every time.
The CLI's default is 3000, so both can run on one machine. If something else
holds 3101, the app says so and does not start.

The config file in the folder above is the only configuration the server has;
see `--data-dir` above.

## The window and the page

The page gets no native access at all: no Tauri command, no capability, no
plugin permission is granted to it. Everything the app does, it decides from
outside the page.

- Only two origins load in the window: the server's, and the app's own
  starting page. Any other web page opens in the default browser, and anything
  that is not a web page is refused. `target="_blank"` and `window.open` never
  make a second window.
- The camera and the microphone are granted to the server's page and to
  nothing else. The operating system still asks before the app first gets
  either; on macOS the reasons shown are in `desktop/Info.plist`.
- A download, such as an exported report, is saved to the Downloads folder
  under the name the page offered, numbered rather than overwriting.

Closing the window, or quitting from the menu or the dock, asks the page's own
leave-page guard (`web/before-unload.js`) whether an interview's report is
still unsaved. If it is, the app asks before quitting; if the page does not
answer within five seconds, it asks as well. When the interview is in the
browser instead (see below) the app cannot ask that page, so it always asks.
Then it closes the server's stdin, gives it five seconds, and kills it if it is
still there.

## Linux and WebRTC

The interview is a LiveKit call, which is WebRTC. On Linux the window is
WebKitGTK, where WebRTC is a build-time option that a distribution's package may
leave off. Without it the page loads, takes the camera, and then cannot connect.

So the app's starting page reports whether `RTCPeerConnection` exists before
it shows the interview. If it does not, the app opens the interview in the
default browser instead, keeps the server running, and its window says so,
with a link to open the browser again. The browser and the window are separate
origins as far as stored history goes: a machine that uses the browser route
keeps its history in that browser.

macOS (WKWebView) and Windows (WebView2) have WebRTC, so this only changes
anything on Linux.

## What the desktop app does not do yet

- GitHub sign-in. The OAuth page opens in the browser, and the session it
  creates lands there, not in the window. A config with `GITHUB_CLIENT_ID` and
  `GITHUB_CLIENT_SECRET` set is a deployment for other people, which is what the
  CLI is for.
- Recording, which needs that sign-in (see
  [recording-contract.md](recording-contract.md)).
- Signed installers. Until they are signed, macOS refuses the first launch as
  from an unidentified developer (allow it under System Settings, Privacy &
  Security), and Windows SmartScreen warns.
- Automatic updates. A new version is a new installer.

None of this makes an interview work offline: LiveKit and Gemini are still
needed, and C, C++ and Java runs still go to Compiler Explorer.

## What has not been qualified

The checks in the gate and the smoke test below prove the app starts its
server, serves only to its own origin, and does not leave the server behind.
They do not prove an interview works in a given web view. Before a release is
called more than a preview, each platform needs these run on real hardware,
with the outcome recorded per case:

1. The camera and microphone prompts appear, and granting them works.
2. The media preflight completes: the meter moves, the face check runs or says
   it is unavailable.
3. The interviewer is heard, and the candidate's speech is transcribed.
4. Speaking over the interviewer interrupts it.
5. A Python test run passes.
6. Ending the interview renders the report, and its save is confirmed.
7. Quitting and relaunching lists that report in history.

Then closing the window and quitting from the menu, in each phase of an
interview, with the report-saved state checked after each. A Chromium run
under Playwright says nothing about WKWebView, WebView2 or WebKitGTK.

## Building it

The server first, since the app needs one to start:

```sh
make build                                      # target/release/codetrial
```

On Linux, the app also needs WebKitGTK 4.1 and its friends (Debian and Ubuntu
names; Ubuntu 22.04 is the oldest with 4.1):

```sh
sudo apt-get install libwebkit2gtk-4.1-dev libxdo-dev libssl-dev librsvg2-dev
```

Everything the app's build produces lands in `target/desktop/`, beside the
server's own `target/`: `desktop/.cargo/config.toml` says so for cargo and the
Tauri CLI run from `desktop/`, and `scripts/test.sh` passes the same directory
when it builds from the root. Nothing is written under `desktop/` itself.

A development run, pointing the app at that server with
`CODETRIAL_DESKTOP_SERVER` because a development build has no bundle for it to
sit in:

```sh
cd desktop
CODETRIAL_DESKTOP_SERVER=$PWD/../target/release/codetrial cargo run
```

Installers come from the Tauri CLI, with the server copied to the name it
expects under `target/desktop/sidecar/`. `tauri.bundle.json` is what adds the
server to the bundle; it is kept out of `tauri.conf.json` so that a plain
`cargo build` of the app does not need a server built first.

```sh
triple=$(rustc -vV | sed -n 's/^host: //p')
mkdir -p target/desktop/sidecar
cp target/release/codetrial "target/desktop/sidecar/codetrial-$triple"
cd desktop
npx @tauri-apps/cli@2.12.3 build --config tauri.bundle.json
```

The installers are under `target/desktop/release/bundle/`. Linux gets `.deb`,
`.rpm` and AppImage; macOS an `.app` in a `.dmg`; Windows an MSI and an NSIS
installer. Each puts `codetrial` beside `codetrial-desktop`,
so the `.deb` and `.rpm` also put the CLI on the `PATH`.

## Checks

`scripts/test.sh` reaches the app through two gates, because the root's
`fmt`, `clippy` and `cargo test` never see a crate with its own manifest:
`desktop-fmt` always, and `desktop` (clippy and the unit tests) where the
platform's web view can be linked, which on Linux means `libwebkit2gtk-4.1-dev`
is installed. Without it the gate skips and says so. The formatter chain and
`cargo audit` cover `desktop/` as well.

`scripts/desktop-smoke.sh APP` is outside the gate because it needs a built app
and a display. It starts the app, checks that its server answers with the Setup
page and refuses a foreign `Host`, then kills the app with SIGKILL and checks
the server went with it. On Linux, run it under `xvfb-run -a`.

`.github/workflows/desktop.yml` builds the installers on all three platforms
and runs the smoke test against each build, for pull requests that touch the
app and for pushes to `main` that do. A tag `desktop-vX.Y.Z` additionally
publishes them as a prerelease, after checking that `X.Y.Z` is the version in
`desktop/Cargo.toml`. The CLI's rolling `latest` release is not touched.
