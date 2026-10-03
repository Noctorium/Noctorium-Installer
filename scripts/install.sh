#!/bin/sh
# Installs Noctorium on Linux, from a terminal:
#
#   curl -fsSL https://noctorium.vercel.app/install | sh
#
# and with options, which go after sh -s --:
#
#   curl -fsSL https://noctorium.vercel.app/install | sh -s -- --product cli --yes
#
# It fetches the terminal installer, noctorium-installer-cli-linux-x64, from the latest release, checks it
# against the SHA256SUMS.txt published beside it, and runs it with those options. The installer then says
# what it found and asks before it changes anything; --help lists what else it takes. Its exit status is
# this script's: 0 when it did what was asked, 1 when it did not, 2 when the options were not understood.
#
# Plain POSIX sh, because it is piped into whatever sh is -- dash, on Debian and Ubuntu. Everything happens
# in main, called on the last line, so a download cut short defines half a function and runs nothing.

asset=noctorium-installer-cli-linux-x64
sums=SHA256SUMS.txt

main() {
    colour "$@"

    case $(uname -s) in
        Linux) ;;
        Darwin)
            fail "There is no macOS build of Noctorium yet."
            return 1 ;;
        MINGW* | MSYS* | CYGWIN*)
            fail "This is the installer for Linux. On Windows, in PowerShell: irm https://noctorium.vercel.app/install | iex"
            return 1 ;;
        *)
            fail "Noctorium is built for Linux and Windows, and this is $(uname -s)."
            return 1 ;;
    esac
    case $(uname -m) in
        x86_64 | amd64) ;;
        *)
            fail "Noctorium is built for x86_64 only, and this machine is $(uname -m)."
            return 1 ;;
    esac

    repository=${NOCTORIUM_REPOSITORY:-Noctorium/Noctorium-Installer}
    case $repository in
        */*/* | /* | */ | *[!A-Za-z0-9._/-]*) repository= ;;
        */*) ;;
        *) repository= ;;
    esac
    if [ -z "$repository" ]; then
        fail "NOCTORIUM_REPOSITORY should be owner/name, such as Noctorium/Noctorium-Installer, not \"$NOCTORIUM_REPOSITORY\"."
        return 1
    fi
    # The release's own file links rather than the API, so GITHUB_TOKEN is not needed here; the installer
    # reads it for the questions it asks the API.
    base="https://github.com/$repository/releases/latest/download"

    if ! have curl && ! have wget; then
        fail "This needs curl or wget to download the installer, and neither is here."
        return 1
    fi
    # Checked before anything is downloaded: a file that cannot be checked is not run.
    if ! have sha256sum && ! have shasum && ! have openssl; then
        fail "Nothing here computes a SHA-256 -- sha256sum, shasum or openssl -- so the download could not be checked."
        return 1
    fi

    if ! tmp=$(mktemp -d 2>/dev/null) || [ -z "$tmp" ]; then
        fail "Could not make a temporary folder to download into."
        return 1
    fi
    trap 'rm -rf "$tmp"' EXIT
    trap 'rm -rf "$tmp"; exit 129' HUP
    trap 'rm -rf "$tmp"; exit 130' INT
    trap 'rm -rf "$tmp"; exit 143' TERM

    printf '\n'
    say "Fetching the Noctorium installer from $repository, the latest release..."
    for name in "$asset" "$sums"; do
        if ! download "$base/$name" "$tmp/$name"; then
            fail "Could not download $base/$name"
            return 1
        fi
    done

    # Lines of "<sha256>  <name>", as sha256sum writes them; a star before the name means binary mode.
    expected=$(tr -d '\r' < "$tmp/$sums" | awk -v name="$asset" '
        { file = $2; sub(/^\*/, "", file) }
        file == name && length($1) == 64 && $1 ~ /^[0-9A-Fa-f]+$/ { print tolower($1); exit }')
    if [ -z "$expected" ]; then
        fail "$sums in that release does not list $asset, so it cannot be checked, and has not been run."
        return 1
    fi
    actual=$(sha256 "$tmp/$asset" | tr 'A-F' 'a-f')
    if [ "$actual" != "$expected" ]; then
        rm -f "$tmp/$asset"
        fail "$asset does not match $sums, so it has been deleted rather than run."
        say "expected $expected"
        say "received ${actual:-nothing}"
        return 1
    fi
    good "Checked against $sums: it matches."
    chmod +x "$tmp/$asset"

    # Piped from curl, standard input is this script, not the keyboard, and the installer's questions
    # would read the end of it. The terminal is still there as /dev/tty -- when there is one: [ -r ] says
    # yes even without one, so the test is whether it can actually be opened. With no terminal at all, in
    # CI or a container, it runs as it is, and needs --yes for anything it would otherwise ask.
    if [ ! -t 0 ] && (: < /dev/tty) 2>/dev/null; then
        "$tmp/$asset" "$@" < /dev/tty
    else
        "$tmp/$asset" "$@"
    fi
    status=$?
    # 126 is the shell failing to run it at all, which a /tmp mounted noexec does.
    if [ "$status" -eq 126 ]; then
        fail "The installer could not be run from $tmp. If that is mounted noexec, set TMPDIR to a folder that is not, and try again."
    fi
    return "$status"
}

# Colour only for a terminal, and not when NO_COLOR is set or --no-color was given.
colour() {
    dim='' green='' red='' reset=''
    if [ ! -t 1 ] || [ -n "${NO_COLOR:-}" ]; then
        return 0
    fi
    for argument in "$@"; do
        case $argument in --no-color | --no-colour) return 0 ;; esac
    done
    dim=$(printf '\033[2m') green=$(printf '\033[32m') red=$(printf '\033[1;31m') reset=$(printf '\033[0m')
}

say() { printf '  %s%s%s\n' "$dim" "$1" "$reset"; }
good() { printf '  %s%s%s\n' "$green" "$1" "$reset"; }
# To standard error, which is coloured only when it is a terminal as well.
fail() {
    if [ -t 2 ]; then
        printf '\n  %s%s%s\n' "$red" "$1" "$reset" >&2
    else
        printf '\n  %s\n' "$1" >&2
    fi
}

have() { command -v "$1" > /dev/null 2>&1; }

# curl retries what is worth retrying -- a 5xx, a timeout -- and is held to https, redirects included.
download() {
    if have curl; then
        curl -fsSL --proto '=https' --tlsv1.2 --retry 2 -o "$2" "$1"
    else
        wget -q -O "$2" "$1"
    fi
}

# Read from standard input, so no tool has a file name to quote or escape in what it prints back.
sha256() {
    if have sha256sum; then
        sha256sum < "$1" | cut -d ' ' -f 1
    elif have shasum; then
        shasum -a 256 < "$1" | cut -d ' ' -f 1
    else
        openssl dgst -sha256 < "$1" | sed 's/^.*= *//'
    fi
}

main "$@"
