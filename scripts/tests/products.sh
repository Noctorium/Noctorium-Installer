#!/bin/sh
# What install.sh asks the installer for, tried without a network: its choose_products and has_stats,
# taken out of the script as they are, against the checksums of a release from before Noctorium Stats --
# 0.12.2, which is what "latest" is the day the script goes live -- and of one with it.
#
#   sh scripts/tests/products.sh
#
# Each line is what was given, and then the --product the installer would be run with, whether Stats was
# left out, and whether that left nothing to install. Exits 1 if any is not what it should be.
set -eu
script=${1:-scripts/install.sh}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
sed -n '/^choose_products() {$/,/^}$/p; /^has_stats() {$/,/^}$/p' "$script" > "$work/functions.sh"
# shellcheck source=/dev/null
. "$work/functions.sh"

hash=0000000000000000000000000000000000000000000000000000000000000000
for name in noctorium_0.12.2_amd64.deb Noctorium-0.12.2.apk Noctorium-0.12.2-macos-arm64.dmg \
    noctorium-cli-0.12.2-linux-x64.tar.gz noctorium-installer-cli-linux-x64 noctorium-installer-cli-macos; do
    printf '%s  %s\n' "$hash" "$name"
done > "$work/0.12.2.txt"
# With Stats, and the Windows line endings a checksum file may have come with.
cp "$work/0.12.2.txt" "$work/0.13.0.txt"
for name in noctorium-stats-0.13.0-linux-x64.tar.gz Noctorium-Stats-0.13.0.apk; do
    printf '%s  %s\r\n' "$hash" "$name"
done >> "$work/0.13.0.txt"
# Stats for a Mac and nothing else, in binary mode as some tools write it.
cp "$work/0.12.2.txt" "$work/mac-only.txt"
printf '%s  *%s\n' "$hash" noctorium-stats-0.13.0-macos-arm64.zip >> "$work/mac-only.txt"

wrong=0
# Set by choose_products, which ShellCheck cannot see from here.
product='' stats_left_out='' nothing_left=''
check() {
    expected="$1|$2|$3"
    shift 3
    choose_products "$@"
    got="$product|$stats_left_out|$nothing_left"
    shift 2
    if [ "$got" = "$expected" ]; then
        echo "ok     ${NOCTORIUM_PRODUCT:+NOCTORIUM_PRODUCT=$NOCTORIUM_PRODUCT }$*  ->  $got"
    else
        echo "WRONG  ${NOCTORIUM_PRODUCT:+NOCTORIUM_PRODUCT=$NOCTORIUM_PRODUCT }$*  ->  $got, not $expected"
        wrong=$((wrong + 1))
    fi
}

unset NOCTORIUM_PRODUCT
# A release from before Stats: left out, said so, and the rest asked for in words its installer knows.
check both 1 '' Linux "$work/0.12.2.txt" --dry-run --product all --yes
check '' 1 1 Linux "$work/0.12.2.txt" --product stats
check desktop 1 '' Linux "$work/0.12.2.txt" --product=desktop,stats
check '' 1 1 Darwin "$work/0.12.2.txt" --product stats
# What every installer has always taken goes through as it was.
check both '' '' Linux "$work/0.12.2.txt" --product both
check cli '' '' Linux "$work/0.12.2.txt" --product CLI
# Left to the installer: nothing asked, something it will refuse, or something that installs nothing.
check '' '' '' Linux "$work/0.12.2.txt" --yes
check '' '' '' Linux "$work/0.12.2.txt" --product nonsense
check '' '' '' Linux "$work/0.12.2.txt" --product stats --help
check '' '' '' Linux "$work/0.12.2.txt" --product
check '' '' '' Linux "$work/0.12.2.txt" --product --yes
# A release with Stats, for this system or not.
check 'cli,stats' '' '' Linux "$work/0.13.0.txt" --product cli+stats
check 'desktop,cli,stats' '' '' Linux "$work/0.13.0.txt" --product all
check stats '' '' Linux "$work/0.13.0.txt" --product=stats
check '' 1 1 Darwin "$work/0.13.0.txt" --product stats
check stats '' '' Darwin "$work/mac-only.txt" --product stats
# NOCTORIUM_PRODUCT, which --product wins over.
NOCTORIUM_PRODUCT=stats
check '' 1 1 Linux "$work/0.12.2.txt" --yes
check cli '' '' Linux "$work/0.12.2.txt" --product cli
NOCTORIUM_PRODUCT='desktop, stats'
check 'desktop,stats' '' '' Linux "$work/0.13.0.txt"
check desktop 1 '' Linux "$work/0.12.2.txt"
NOCTORIUM_PRODUCT='*'
check '' '' '' Linux "$work/0.12.2.txt"
unset NOCTORIUM_PRODUCT

echo "$wrong wrong"
test "$wrong" = 0
