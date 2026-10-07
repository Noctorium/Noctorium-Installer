#!/bin/sh
# Installs Noctorium on Linux or a Mac, from a terminal:
#
#   curl -fsSL https://noctorium.vercel.app/install | sh
#
# and with options, which go after sh -s --:
#
#   curl -fsSL https://noctorium.vercel.app/install | sh -s -- --product cli --yes
#
# It fetches the terminal installer from the latest release -- noctorium-installer-cli-linux-x64 on Linux,
# noctorium-installer-cli-macos on a Mac, one file for Apple silicon and Intel alike -- checks it against
# the SHA256SUMS.txt published beside it, and runs it with those options. The installer then says what it
# found and asks before it changes anything; --help lists what else it takes. Its exit status is this
# script's: 0 when it did what was asked, 1 when it did not, 2 when the options were not understood.
#
# It offers Noctorium, the Noctorium CLI and Noctorium Stats, which shows what you have listened to:
# --product desktop, cli, stats, both (Noctorium and the CLI) or all, or several joined by commas, such as
# desktop,stats. NOCTORIUM_PRODUCT in the environment says the same, for when options are awkward:
#
#   curl -fsSL https://noctorium.vercel.app/install | NOCTORIUM_PRODUCT=stats sh
#
# Noctorium Stats is new, and a release from before it has none, nor an installer that knows its name. So
# when Stats is asked for, this looks in SHA256SUMS.txt first: if the release has no Stats for this
# machine, it is left out, said so, and the rest is installed as the installer knows it -- nothing is
# downloaded for Stats, and nothing fails for want of it.
#
# Plain POSIX sh, because it is piped into whatever sh is -- dash on Debian and Ubuntu, and on a Mac a bash
# from 2006 standing in for it. Everything happens in main, called on the last line, so a download cut
# short defines half a function and runs nothing.

sums=SHA256SUMS.txt

main() {
    colour "$@"

    system=$(uname -s)
    case $system in
        Linux)
            case $(uname -m) in
                x86_64 | amd64) ;;
                *)
                    fail "Noctorium is built for x86_64 only, and this machine is $(uname -m)."
                    return 1 ;;
            esac
            asset=noctorium-installer-cli-linux-x64 ;;
        Darwin)
            # One file for every Mac: a universal binary, which runs natively on Apple silicon and on Intel
            # alike, so uname -m is not asked -- and under Rosetta it would say x86_64 on an Apple silicon
            # Mac anyway. Which Noctorium the Mac gets is the installer's to work out, from the kernel.
            asset=noctorium-installer-cli-macos ;;
        MINGW* | MSYS* | CYGWIN*)
            fail "This is the installer for Linux and macOS. On Windows, in PowerShell: irm https://noctorium.vercel.app/install | iex"
            return 1 ;;
        *)
            fail "Noctorium is built for Linux, macOS and Windows, and this is $system."
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
    # The checksums first: they say whether the release has Noctorium Stats, which decides what the
    # installer is asked for -- and whether there is anything to fetch it for at all.
    if ! download "$base/$sums" "$tmp/$sums"; then
        fail "Could not download $base/$sums"
        return 1
    fi

    choose_products "$system" "$tmp/$sums" "$@"
    if [ -n "$stats_left_out" ]; then
        say "Noctorium Stats is not in the latest release yet, so it is left out, and nothing is downloaded for it."
    fi
    if [ -n "$nothing_left" ]; then
        say "Nothing else was asked for, so nothing has been installed."
        return 0
    fi
    if [ -n "$product" ]; then
        # The options again, with --product said once, as worked out: whatever --product said before, and
        # however it said it, goes, and NOCTORIUM_PRODUCT is passed on as an option, which an installer
        # from before it reads where it would not read the variable.
        skip=
        for argument do
            shift
            if [ -n "$skip" ]; then
                skip=
                continue
            fi
            case $argument in
                --product) skip=1; continue ;;
                --product=*) continue ;;
            esac
            set -- "$@" "$argument"
        done
        set -- "$@" --product "$product"
    fi

    say "Fetching the Noctorium installer from $repository, the latest release..."
    if ! download "$base/$asset" "$tmp/$asset"; then
        fail "Could not download $base/$asset"
        return 1
    fi

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
    # curl and wget do not set the quarantine flag a browser does, so on a Mac there is normally nothing to
    # take off. If there is, Gatekeeper would refuse to run the installer -- it has no Developer ID behind
    # it, only the checksum just checked -- so it comes off, and its absence is not worth a word.
    if [ "$system" = Darwin ]; then
        xattr -d com.apple.quarantine "$tmp/$asset" 2> /dev/null || :
    fi

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

# What to ask the installer for, from what was asked -- the last --product in the options, or
# NOCTORIUM_PRODUCT -- and what the release has for this system ($1), as its checksums ($2) list it. Sets:
#
#   product         the --product to run the installer with, or nothing to pass the options on unchanged
#   stats_left_out  set when Noctorium Stats was asked for and the release has none for this machine
#   nothing_left    set when that was all that was asked for, and there is nothing to run the installer for
#
# Unchanged when nothing was asked for, which is the installer's own question to ask; when what was asked
# is not something this knows, which is the installer's to refuse, with its exit status of 2; and when the
# options ask for help, a version or a listing, which install nothing.
choose_products() {
    product='' stats_left_out='' nothing_left=''
    for_system=$1 listing=$2
    shift 2
    asked=${NOCTORIUM_PRODUCT:-} next=''
    for argument do
        if [ -n "$next" ]; then
            asked=$argument next=''
            continue
        fi
        case $argument in
            -h | --help | -V | --about | --list) return 0 ;;
            --product) next=1; asked='' ;;
            --product=*) asked=${argument#--product=} ;;
        esac
    done
    # --product with nothing after it is a mistake for the installer to point out.
    if [ -n "$next" ] || [ -z "$asked" ]; then
        return 0
    fi
    case $asked in -*) return 0 ;; esac

    desktop='' cli='' stats='' known=1
    words=$(printf '%s' "$asked" | tr 'A-Z,+' 'a-z  ')
    # Split on spaces and nothing else: a * in what somebody typed is not a list of files.
    set -f
    for word in $words; do
        case $word in
            desktop) desktop=1 ;;
            cli) cli=1 ;;
            stats) stats=1 ;;
            both) desktop=1 cli=1 ;;
            all) desktop=1 cli=1 stats=1 ;;
            *) known='' ;;
        esac
    done
    set +f
    if [ -z "$known" ]; then
        return 0
    fi
    if [ -n "$stats" ] && ! has_stats "$for_system" "$listing"; then
        stats='' stats_left_out=1
    fi
    # Joined with commas, which only an installer that knows Noctorium Stats reads -- and Stats is only
    # still here when the release, and so its installer, has it. Without Stats, the words every installer
    # has always taken.
    if [ -n "$stats" ]; then
        product=${desktop:+desktop,}${cli:+cli,}stats
    elif [ -n "$desktop" ] && [ -n "$cli" ]; then
        product=both
    elif [ -n "$desktop" ]; then
        product=desktop
    elif [ -n "$cli" ]; then
        product=cli
    else
        nothing_left=1
    fi
}

# Whether the checksums ($2) list a Noctorium Stats for this system ($1): the Linux archive, or either of
# the Mac's, since which one a Mac takes is the installer's to work out.
has_stats() {
    case $1 in
        Darwin) shape='^noctorium-stats-[0-9][^/]*-macos-(arm64|x64)[.]zip$' ;;
        *) shape='^noctorium-stats-[0-9][^/]*-linux-x64[.]tar[.]gz$' ;;
    esac
    tr -d '\r' < "$2" | awk -v shape="$shape" '
        { file = $2; sub(/^\*/, "", file) }
        file ~ shape { found = 1 }
        END { exit found ? 0 : 1 }'
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

# Read from standard input, so no tool has a file name to quote or escape in what it prints back. A Mac
# has shasum, and no sha256sum unless somebody installed one; both print the hash first, then a space.
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
