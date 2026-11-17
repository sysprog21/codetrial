#!/bin/sh

# The one place that says whether this machine can run the gate, so a missing
# tool is named once, before the run starts, with the command that fixes it,
# instead of surfacing as the same build-script error in every gate that needs
# it. scripts/test.sh runs it as its first step and takes its optional-tool skip
# lines from it.
#
# It only looks. It never downloads or installs anything and never runs
# scripts/fetch-clang.sh or scripts/verify-vendor.sh: scripts/test.sh provisions
# the compiler itself and already hashes the vendored assets once per run, which
# a second pass over twelve megabytes of wasm would only repeat.
#
# Exit status: 0 every required check passed, 1 one failed, 2 a bad invocation
# or a repository this script cannot read.
set -eu

usage()
{
    echo "usage: ${0##*/} [--profile local-rust|ci-rust|portable|browser]" \
        "[--format text|json] [--will-provision]" >&2
    exit 2
}

# Resolved as scripts/fetch-clang.sh does, and entered so that `rustc -V` and
# `cargo -V` see the repository's rust-toolchain.toml exactly as they will when
# scripts/test.sh runs them.
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

profile=local-rust
format=text
provision=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        --profile)
            [ "$#" -ge 2 ] || usage
            profile=$2
            shift 2
            ;;
        --format)
            [ "$#" -ge 2 ] || usage
            format=$2
            shift 2
            ;;

        # Passed only by scripts/test.sh, which fetches Clang itself after these
        # checks; run by hand, a missing compiler is still a failure.
        --will-provision)
            provision=true
            shift
            ;;
        *) usage ;;
    esac
done
case "$profile" in
    local-rust | ci-rust | portable | browser) ;;
    *) usage ;;
esac
case "$format" in
    text | json) ;;
    *) usage ;;
esac

# rustup installs the toolchain a rust-toolchain.toml names the first time any
# rustup command runs inside that directory, `rustup toolchain list` included.
# So whether the pinned toolchain exists is asked from outside the repository,
# before any cargo or rustc runs in it, and nothing runs when it is missing.
# This setting stops the same install on a rustup that honours it; the check
# above does not depend on it.
RUSTUP_AUTO_INSTALL=0
export RUSTUP_AUTO_INSTALL
toolchain_ready=true

# The byte whose hex value is $1, written out as is: printf takes it as an octal
# escape in its format, which is the portable way to emit one byte.
raw_byte()
{
    octal=$(printf '%03o' "0x$1")
    # shellcheck disable=SC2059
    printf "\\$octal"
}

# Every JSON string goes through here. Nothing is rejected: a quote, a backslash
# and every control character are escaped, so a hostile environment variable
# cannot break out of the object that reports it.
json_string()
{
    value=$1

    # The plain case needs two substitutions; only a value that carries a
    # control character pays for the byte-by-byte walk.
    case $value in
        *[[:cntrl:]]*) ;;
        *)
            printf '"%s"' "$(printf '%s' "$value" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')"
            return 0
            ;;
    esac
    escaped=
    for byte in $(printf '%s' "$value" | od -An -v -tx1); do
        case $byte in
            22) piece='\"' ;;
            5c) piece="\\\\" ;;
            0a) piece='\n' ;;
            7f) piece='\u007f' ;;
            09) piece='\t' ;;
            0d) piece='\r' ;;
            0[0-9a-f] | 1[0-9a-f]) piece="\\u00$byte" ;;

            # The byte itself, for printable ASCII and for the bytes of a
            # multi-byte character, which JSON carries as they are.
            *) piece=$(raw_byte "$byte") ;;
        esac
        escaped=$escaped$piece
    done
    printf '"%s"' "$escaped"
}

kernel=$(uname -s)
failed=false
required_json=
skipped_json=

say()
{
    [ "$format" = text ] || return 0
    printf '%s\n' "$*"
}

# required NAME OK FOUND WANT MESSAGE
required()
{
    if [ "$2" = true ]; then
        say "ok: $1: $3"
    else
        failed=true
        say "fail: $1: $5"
    fi
    [ "$format" = json ] || return 0
    entry="{\"name\":$(json_string "$1"),\"ok\":$2,\"found\":$(json_string "$3"),\"want\":$(json_string "$4")}"
    required_json=${required_json:+$required_json,}$entry
}

skipped()
{
    say "skipped: $1: $2"
    [ "$format" = json ] || return 0
    entry=$(json_string "$1: $2")
    skipped_json=${skipped_json:+$skipped_json,}$entry
}

have()
{
    command -v "$1" > /dev/null 2>&1
}

rust_install_hint()
{
    case "$kernel" in
        MINGW* | MSYS* | CYGWIN*)
            printf '%s' "download and run rustup-init.exe from https://rustup.rs, then run ./scripts/env-check.sh again"
            ;;
        *)
            printf '%s' "install Rust with: curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh, then run ./scripts/env-check.sh again"
            ;;
    esac
}

# The pin is read the way scripts/test.sh reads other files, with sed: the
# gate's own Python is not required for the browser lane, and this script should
# not need it either.
pinned_channel()
{
    sed -n '/^[[:space:]]*channel[[:space:]]*=/{s/^[^=]*=[[:space:]]*"\([^"]*\)".*/\1/p;q;}' \
        "$ROOT/rust-toolchain.toml" 2> /dev/null || :
}

# With rustup on PATH, whether the pinned toolchain is installed, from a
# directory that has no pin so the question itself installs nothing. Without
# rustup (a distribution or Nix toolchain) the version check below decides.
check_pinned_toolchain()
{
    channel=$(pinned_channel)
    if [ -z "$channel" ]; then
        echo "${0##*/}: no channel in rust-toolchain.toml; cannot tell which toolchain is wanted" >&2
        exit 2
    fi
    have rustup || return 0
    if ! installed=$(cd / && rustup toolchain list); then
        echo "${0##*/}: rustup toolchain list failed" >&2
        exit 2
    fi
    found=false
    while read -r name _; do
        case $name in
            "$channel" | "$channel"-*) found=true ;;
        esac
    done << LIST
$installed
LIST
    if [ "$found" = true ]; then
        required toolchain true "$channel is installed" "$channel installed" ""
    else
        toolchain_ready=false
        required toolchain false "$channel is not installed" "$channel installed" \
            "the toolchain $channel that rust-toolchain.toml pins is not installed; run: rustup toolchain install $channel --profile minimal -c clippy -c rustfmt, then run ./scripts/env-check.sh again"
    fi
}

check_cargo()
{
    if ! have cargo; then
        required cargo false "not found" "on PATH" "not found; $(rust_install_hint)"
        return 0
    fi
    if [ -n "${RUSTUP_TOOLCHAIN:-}" ]; then
        required RUSTUP_TOOLCHAIN false "$RUSTUP_TOOLCHAIN" unset \
            "set to $RUSTUP_TOOLCHAIN, which overrides the rust-toolchain.toml pin; unset it"
        # A rustup shim would act on the override, which may not be installed.
        if have rustup; then
            toolchain_ready=false
        fi
    else
        required RUSTUP_TOOLCHAIN true "unset" unset ""
    fi
    [ "$toolchain_ready" = true ] || return 0
    if cargo_line=$(cargo -V 2>&1); then
        required cargo true "$cargo_line" "on PATH" ""
    else
        required cargo false "$cargo_line" "on PATH" \
            "cargo -V failed: $cargo_line"
    fi
}

check_rustc()
{
    have cargo || return 0
    [ "$toolchain_ready" = true ] || return 0
    if ! have rustc; then
        required rustc false "not found" "$channel" "not found; $(rust_install_hint)"
        return 0
    fi
    rustc_path=$(command -v rustc)
    # The failure text starts with "rustc", so it must not reach the parse.
    rustc_ran=true
    if rustc_line=$(rustc -V 2>&1); then
        rustc_version=$(printf '%s\n' "$rustc_line" | sed -n '1s/^rustc \([^ ]*\).*/\1/p')
    else
        rustc_line="rustc -V failed: $rustc_line"
        rustc_version=
        rustc_ran=false
    fi
    case $channel in
        [0-9]*.[0-9]*.[0-9]*)
            if [ "$rustc_version" = "$channel" ]; then
                required rustc true "$rustc_version" "$channel" ""
            else
                required rustc false "${rustc_version:-$rustc_line}" "$channel" \
                    "${rustc_version:+rustc }${rustc_version:-$rustc_line} is not the $channel that rust-toolchain.toml pins; run from the repository root, or install it with: rustup toolchain install $channel"
            fi
            ;;
        *)
            # A named channel such as nightly has no number to compare.
            if [ "$rustc_ran" = true ]; then
                required rustc true "${rustc_version:-$rustc_line}" "$channel" ""
            else
                required rustc false "$rustc_line" "$channel" "$rustc_line"
            fi
            ;;
    esac

    # A distribution or Nix compiler is legitimate when it is the pinned
    # version; only the version decides the verdict.
    outside=true
    for home in "${CARGO_HOME:-}" "${HOME:-}/.cargo"; do
        [ -n "$home" ] || continue
        case $rustc_path in
            "$home"/bin/*) outside=false ;;
        esac
    done
    if [ "$outside" = true ] && [ "$rustc_version" = "$channel" ]; then
        say "warning: rustc is $rustc_path, not a rustup shim under \$CARGO_HOME/bin or ~/.cargo/bin; the pin is met by version only"
    fi
}

check_component()
{
    # $1 the cargo subcommand, $2 the component rustup names it by.
    have cargo || return 0
    [ "$toolchain_ready" = true ] || return 0
    if cargo "$1" --version > /dev/null 2>&1; then
        required "$2" true "cargo $1 --version" "installed" ""
    else
        required "$2" false "missing" "installed" \
            "cargo $1 is not available; run: rustup component add clippy rustfmt"
    fi
}

check_compiler()
{
    # webrtc-sys needs Clang 21 or later on Linux only; another platform builds
    # with its own compiler, whose version number says nothing here.
    [ "$kernel" = Linux ] || return 0
    cxx=${CXX:-}
    cxx_fetched=false
    if [ -z "$cxx" ]; then
        if [ "$(uname -m)" = x86_64 ]; then
            if [ -x "$ROOT/target/clang/bin/clang++" ]; then
                cxx=$ROOT/target/clang/bin/clang++
                cxx_fetched=true
            elif [ "$provision" = true ]; then
                required CXX true "not set; scripts/test.sh fetches Clang into target/clang" "Clang 21+" ""
                say "note: Clang 21+ is not on disk yet; scripts/test.sh fetches it before the build"
                return 0
            else
                required CXX false "not set" "Clang 21+" \
                    "no Clang 21+; run: sh scripts/fetch-clang.sh target/clang"
                return 0
            fi
        else
            required CXX false "not set" "Clang 21+" \
                "only Linux x86_64 has a fetched Clang; set CXX to a Clang 21+ instead"
            return 0
        fi
    fi

    # A CXX the caller set may carry arguments (a launcher in front of the
    # compiler), so it is split on purpose; the path found under the checkout is
    # one word, whatever the checkout's directory is called.
    if [ "$cxx_fetched" = true ]; then
        cxx_line=$("$cxx" --version 2> /dev/null | sed -n 1p) || cxx_line=
    else
        # shellcheck disable=SC2086
        cxx_line=$($cxx --version 2> /dev/null | sed -n 1p) || cxx_line=
    fi
    cxx_major=$(printf '%s\n' "$cxx_line" | sed -n 's/.*clang version \([0-9][0-9]*\).*/\1/p')
    if [ -n "$cxx_major" ] && [ "$cxx_major" -ge 21 ]; then
        required CXX true "$cxx: clang $cxx_major" "Clang 21+" ""
    else
        required CXX false "$cxx: ${cxx_line:-no version reported}" "Clang 21+" \
            "$cxx is not Clang 21+ (${cxx_line:-no version reported}); set CXX to one, or unset it on Linux x86_64 to use sh scripts/fetch-clang.sh target/clang"
    fi
}

check_vendor()
{
    if [ -d "$ROOT/web/vendor" ]; then
        required web/vendor true present present ""
    else
        required web/vendor false missing present \
            "web/vendor is missing, and the server embeds it; restore it from the checkout"
    fi
}

check_node()
{
    if have node; then
        required node true "$(command -v node)" "on PATH" ""
    else
        required node false "not found" "on PATH" "not found; install Node.js, which runs the generated-file and browser checks"
    fi
}

check_ci_linker()
{
    if have mold; then
        required mold true "$(command -v mold)" "on PATH" ""
    else
        required mold false "not found" "on PATH" "not found; install mold, which the workflow's RUSTFLAGS links with"
    fi
    case ${RUSTFLAGS:-} in
        *-fuse-ld=mold*) required RUSTFLAGS true "${RUSTFLAGS:-}" "-fuse-ld=mold" "" ;;
        *) required RUSTFLAGS false "${RUSTFLAGS:-}" "-fuse-ld=mold" "does not link with mold; the workflow sets -Clink-arg=-fuse-ld=mold" ;;
    esac
}

# `docker info` and not just the client: a client with no daemon behind it
# cannot run the actionlint image, as scripts/test.sh's own fallback decides.
docker_usable()
{
    have docker && docker info > /dev/null 2>&1
}

# The wording is what scripts/test.sh used to print when it probed these tools
# itself, so a skipped check reads the same.
check_optional()
{
    have ruff || skipped ruff "installing it lints the Python under scripts/, tests/ and the skills"
    if ! have shellcheck; then
        set -- "$ROOT"/scripts/*.sh "$ROOT"/.claude/skills/*/*.sh "$ROOT"/tests/stubs/*/*.sh
        skipped shellcheck "installing it checks what $# shell scripts mean, not just that they parse"
    fi
    have dash || skipped dash "\`sh -n\` runs under the system sh, which may accept what dash refuses"
    if ! have actionlint && ! docker_usable; then
        skipped actionlint "installing it, or docker, checks .github/workflows"
    fi
    have cargo-audit || skipped cargo-audit "\`cargo install cargo-audit\` audits the dependency tree"
    have cargo-deny || skipped cargo-deny "\`cargo install cargo-deny\` checks dependency licenses and sources"
}

# What each lane of scripts/test.sh really needs, read off its gate list. Node
# runs generated-file checks in every lane. The portable lane also checks Rust
# formatting, so it needs rustfmt but neither clippy nor a compiler.
case "$profile" in
    local-rust | ci-rust)
        check_pinned_toolchain
        check_cargo
        check_rustc
        check_component clippy clippy
        check_component fmt rustfmt
        check_compiler
        check_vendor
        check_node
        if [ "$profile" = ci-rust ]; then
            check_ci_linker
        fi
        ;;
    portable)
        check_pinned_toolchain
        check_cargo
        check_component fmt rustfmt
        check_node
        ;;
    browser)
        check_node
        ;;
esac
check_optional

if [ "$failed" = true ]; then
    ok=false
else
    ok=true
fi
if [ "$format" = json ]; then
    printf '{"profile":%s,"ok":%s,"required":[%s],"skipped":[%s]}\n' \
        "$(json_string "$profile")" "$ok" "$required_json" "$skipped_json"
else
    say "env-check: profile $profile: $([ "$ok" = true ] && echo ok || echo failed)"
fi
[ "$ok" = true ] || exit 1
exit 0
