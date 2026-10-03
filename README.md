# Noctorium releases

This is where Noctorium is published: the Windows installer, the Android APK, and for Linux a Debian
package, an RPM, an Arch package, an AppImage and a Flatpak, with a `SHA256SUMS.txt` beside them. The
applications check here for updates. The player itself is elsewhere — the desktop is
[Noctorium-Desktop](https://github.com/Noctorium/Noctorium-Desktop), the phone is
[Noctorium-Mobile](https://github.com/Noctorium/Noctorium-Mobile), and what they share is
[Noctorium-Base](https://github.com/Noctorium/Noctorium-Base).

What *is* here, besides the release pipeline, is the small installers, and in `packaging/` the sources
of the Linux formats that jpackage does not make by itself.

## The installers

Small programs that do one thing: ask GitHub what the latest release is, fetch the file for the machine
they are on, check it against the checksum published beside it, and hand it to whatever installs software
there. They are what somebody downloads once. Everything after that is the application's own updater.

From a terminal, not even that: one line fetches the terminal installer from the latest release, checks it
against `SHA256SUMS.txt`, and runs it — and the installer says what it found and asks before it changes
anything.

```powershell
irm https://noctorium.vercel.app/install | iex                # Windows, in PowerShell
```

```bash
curl -fsSL https://noctorium.vercel.app/install | sh          # Linux
```

Options for the installer, [listed below](#the-terminal-installer), go after either. `iex` has no way to
pass any on, so on Windows the line becomes a script block:

```powershell
& ([scriptblock]::Create((irm https://noctorium.vercel.app/install))) --product cli --yes
```

```bash
curl -fsSL https://noctorium.vercel.app/install | sh -s -- --product cli --yes
```

They are `scripts/install.ps1` and `scripts/install.sh`, served from this repository's main branch. Either
refuses to run a download that does not match its checksum, and hands back the installer's exit status —
in PowerShell as `$LASTEXITCODE`, because it returns rather than calling `exit`, which through `iex` would
close the window. `.github/workflows/scripts.yml` runs both against the real latest release whenever they change.

| | Where | Built from |
| --- | --- | --- |
| `Noctorium-Installer-windows-x64.exe` | Windows, in a window | `pc/`, in Rust |
| `Noctorium-Installer-x86_64.AppImage`, or the bare `noctorium-installer-linux-x64` | Linux, in a window | `pc/` |
| `noctorium-installer-cli-windows-x64.exe`, `noctorium-installer-cli-linux-x64` | A terminal, on either | `pc/` |
| `Noctorium-Installer-android.apk` | Android | `phone/`, in Dart with Flutter |

None of them installs a package itself. The PC ones run the Windows installer, or hand the package to the
distribution's own package manager — `apt`, `dnf`, `zypper` or `pacman` — so dependencies are resolved
rather than merely reported. What has no package manager, they put in place themselves, for the person
running them and nobody else: an AppImage in `~/Applications` with a menu entry, a Flatpak bundle handed
to `flatpak install --user`, the Noctorium CLI unpacked into a folder of its own. The phone one hands the
APK to Android's own package installer, which shows its own screen and asks again — and the first time
sends you to a settings page to allow it at all.

All of them refuse to install anything they cannot check. A release with no `SHA256SUMS.txt`, or a
download that does not match the checksum published for it, is deleted rather than run. All of them skip
their own files in a release, so an installer never offers to install itself.

All of them read `GITHUB_TOKEN` from the environment if it is set. This repository is public, so none
needs it to see a release; it only raises the rate limit an address shares with everyone else behind it.

The window ones show what they found, wait to be told to go ahead, and draw a progress bar; it is the
first thing anybody sees of Noctorium, usually before they have any reason to trust it, and a console full
of scrolling text is not what somebody who has just downloaded a music player expects. On Linux the window
has one more row, for the format, which starts on whatever this machine would pick. The window program
still installs in the terminal behind `--cli`, taking every default without asking, and does so by itself
on a machine with no display — over ssh, say, where `DISPLAY` and `WAYLAND_DISPLAY` are both unset.

### The terminal installer

`noctorium-installer-cli` is the same installer for a terminal: cmd, PowerShell and Windows Terminal on
Windows, anything on Linux. It says what it found — the machine, the distribution, its package manager,
the release — asks which product and, on Linux, which format, shows the file and the exact command it is
about to run, and asks once more before it downloads anything. Every question has a default, and `--yes`
takes them all, so the same program serves somebody at a prompt and a script installing Noctorium on a
row of machines.

```
  ♫ Noctorium  installer 1.2.0

  System    Ubuntu 24.04.1 LTS · x86-64 · apt, flatpak
  Release   v0.7.0 (the latest)

  What would you like to install?
    1  Noctorium      the music player, in a window  recommended
    2  Noctorium CLI  the same player, in a terminal
    3  Both
  Choose 1-3 [1]
```

| Option | |
| --- | --- |
| `-y`, `--yes` | Ask nothing: take the defaults and install. |
| `--product desktop\|cli\|both` | Noctorium (the default), the Noctorium CLI, or both. |
| `--format auto\|deb\|rpm\|arch\|appimage\|flatpak` | Linux only. `auto` is the distribution's own package where there is one, and the AppImage everywhere else. |
| `--version X.Y.Z` | Install release `vX.Y.Z` rather than the latest. |
| `--dry-run` | Show what would be downloaded and run, and stop there. |
| `--list` | List the release's files and their sizes, marking the ones for this machine. |
| `--no-color` | Plain text. `NO_COLOR` does the same, and output that is not a terminal is never coloured. |
| `-V`, `--about` | This installer's own version. |
| `-h`, `--help` | The options. |

It exits 0 when it did what was asked, 1 when it did not — including when the answer to "Install?" was
no — and 2 when the options could not be understood, so a script can tell a typo from a failed install.
Asked a question with nothing to answer it — standard input closed, and no `--yes` — it stops with 2
rather than guessing.

```bash
noctorium-installer-cli                                  # ask, then install
noctorium-installer-cli --yes                            # Noctorium, the way this machine prefers
noctorium-installer-cli --product both --yes             # and the Noctorium CLI beside it
noctorium-installer-cli --format appimage                # the AppImage, whatever the distribution
noctorium-installer-cli --version 0.6.0 --dry-run        # what installing 0.6.0 would do
curl -fsSLo noctorium-installer-cli https://github.com/Noctorium/Noctorium-Installer/releases/latest/download/noctorium-installer-cli-linux-x64 \
  && chmod +x noctorium-installer-cli && ./noctorium-installer-cli
```

The Linux one is linked statically against musl, so it runs on any distribution however old. In a
terminal it asks for administrator rights with `sudo`, which asks right there; `pkexec` needs a polkit
agent running to ask on its behalf, and over ssh there usually is not one. With nobody at the keyboard it
tries `pkexec` first. Run as root it asks for nothing — and warns, if that root came from `sudo`, that an
AppImage, a Flatpak or the CLI would then be installed for root rather than for you.

The **Noctorium CLI** — the player in a terminal, when a release carries it — is installed for you alone:
on Windows into `%LOCALAPPDATA%\Programs\Noctorium CLI`, which is added to your PATH (open a new terminal
afterwards; one already open keeps the old PATH), and on Linux into `~/.local/share/noctorium-cli`, with
`~/.local/bin/noctorium` linked to it. A newer one replaces an older one whole. A release that does not
carry it yet says so, and installs nothing.

### Building them

```bash
cd pc    && cargo test && cargo build --release    # both PC installers
cd phone && flutter test && flutter build apk      # the phone installer

# The terminal installer alone, without the window toolkit, as the release builds it for Linux
cd pc && cargo build --release --no-default-features --bin noctorium-installer-cli --target x86_64-unknown-linux-musl
```

Building the window one on Linux needs the headers it is drawn with, which a desktop usually has already,
and the static terminal one needs musl's compiler wrapper and the target:

```bash
sudo apt-get install libxkbcommon-dev libwayland-dev libgl1-mesa-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev
sudo apt-get install musl-tools && rustup target add x86_64-unknown-linux-musl
```

## Which file

| Platform | File |
| --- | --- |
| Windows | `Noctorium-<version>-windows-x64-setup.exe`, or the `.msi` for deployment |
| Debian, Ubuntu, Mint | `noctorium_<version>_amd64.deb` |
| Fedora, RHEL, openSUSE | `noctorium-<version>.x86_64.rpm` |
| Arch, Manjaro, EndeavourOS | `noctorium-<version>-1-x86_64.pkg.tar.zst` |
| Any Linux | `Noctorium-<version>-x86_64.AppImage` |
| Any Linux with Flatpak | `Noctorium-<version>-x86_64.flatpak` |
| Android | `Noctorium-<version>.apk` |
| The Noctorium CLI | `noctorium-cli-<version>-windows-x64.zip`, `noctorium-cli-<version>-linux-x64.tar.gz`, in the releases that carry it |

On Linux, **take your distribution's own package when there is one**: the package manager installs what
it needs, knows it is there, and removes it cleanly. The Arch package installs to `/opt/noctorium` like the
others, with `noctorium` on PATH; `sudo pacman -U noctorium-<version>-1-x86_64.pkg.tar.zst`.

Anywhere else, the **AppImage** runs as it is: mark it executable and start it, or let the installer put it
in `~/Applications` and add it to the menu. It needs FUSE to mount itself, which most desktops have; where
one does not, `--appimage-extract-and-run` runs it without. To remove it, delete the file and
`~/.local/share/applications/noctorium.desktop`.

The **Flatpak** runs in a sandbox, for one user, and is the one format that needs nothing at all from the
distribution — it brings its own mpv. It is a bundle rather than a Flathub listing, so it is installed
from the file, and its runtime comes from Flathub:

```bash
flatpak remote-add --user --if-not-exists flathub https://dl.flathub.org/repo/flathub.flatpakrepo
flatpak install --user --bundle Noctorium-<version>-x86_64.flatpak
flatpak run app.noctorium.Noctorium
```

Inside it, Noctorium can reach the network, your Music and Downloads folders, the sound server, the
notification and keyring services, and Discord's presence socket — nothing else of your home folder. Its
settings, sign-ins and the unpacked Chromium are kept in `~/.var/app/app.noctorium.Noctorium`, not in
`~/.local/share/noctorium` where the other formats keep them, so moving from one to the other means
signing in again.

The desktop packages carry their own Java runtime and their own Chromium for the sign-in window; the
Windows ones carry mpv and yt-dlp as well, and the Flatpak carries mpv. **Everything else on Linux plays
through the distribution's mpv**: the Arch package depends on it, so pacman brings it, and for the .deb,
the .rpm and the AppImage it is one `sudo apt install mpv`, `sudo dnf install mpv` or the like, which the
installers mention when it is missing. yt-dlp needs nothing: Noctorium fetches its own copy and keeps it
current. **Close Noctorium before installing over it** — the installer cannot replace files the running
program holds open.

## Verifying a download

```
sha256sum --check --ignore-missing SHA256SUMS.txt
```

The APK is signed with Noctorium's release key, certificate SHA-256 fingerprint:

```
46:CF:8B:96:C4:37:49:7A:93:AF:76:2B:91:94:AD:11:09:5D:D6:4A:B6:AB:EA:E9:60:C0:A5:98:C5:63:49:ED
```

A debug-signed build already on a phone (from `gradlew installDebug`) has to be uninstalled first; Android
will not replace it with a release-signed one.

## Cutting a release

The same tag goes on all three repositories, applications first:

```bash
for repo in Noctorium-Desktop Noctorium-Mobile Noctorium-Installer; do
  git -C ../$repo tag v1.2.3 && git -C ../$repo push origin v1.2.3
done
```

The one on this repository is what starts the build, so it goes last. The other two are what the build
checks out: `.github/workflows/release.yml` takes Noctorium-Desktop and Noctorium-Mobile **at that same
tag**, each with the Noctorium-Base commit it pins, runs the tests, packages each platform on its own
runner, and opens a **draft** release with everything attached and the checksums beside it. It is a draft
on purpose — read it, check the files are the sizes you expect, write the notes, and publish it yourself.

Tagging only this repository fails in the first minute, and the message does not say why: the checkout of
an application looks for a ref that is not there and reports nothing but `git failed with exit code 1`.
This paragraph exists because that is exactly how 0.4.4 began.

To build without releasing, run the workflow by hand from the Actions tab and give it a version, and
optionally a branch or commit of each application. The artifacts are attached to the run for two weeks
and no release is created.

### The Linux formats, and trying them

jpackage makes the .deb and the .rpm, and the application folder inside them — a launcher, the jars, a
Java runtime of its own. The other three formats are that same folder wrapped differently, so
`.github/workflows/linux.yml` keeps it from the first job as an artifact called `stage-linux-app` and
builds the rest from it:

| Format | From | Checked by |
| --- | --- | --- |
| Arch package | `packaging/arch/PKGBUILD`, with makepkg in an `archlinux:base-devel` container | `pacman -Qip`, then installing it for real and running `ldd` over every library in it, Chromium's included |
| AppImage | `packaging/appimage/`, with appimagetool | unpacking it with its own runtime and finding the launcher |
| Flatpak | `packaging/flatpak/app.noctorium.Noctorium.yml`, with flatpak-builder against `org.freedesktop.Platform//24.08` | installing the bundle twice, and having its mpv play a track over https |

The Flatpak's manifest builds mpv 0.41 itself, with libplacebo and libass under it, against the runtime's
own FFmpeg: audio out through PulseAudio, and every video output, script engine and disc reader switched
off. Nothing is compiled for the other formats. `stage-*` artifacts are never attached to a release.

`.github/workflows/packaging.yml` runs all of it — the Linux formats and both PC installers — without the
tests, the Windows installer, the APK or a release. Push a branch named `packaging/<anything>`, or run it
by hand with a branch or commit of Noctorium-Desktop; the files are attached to the run.

### What the workflow needs

| Secret | What it is |
| --- | --- |
| `ANDROID_KEYSTORE_BASE64` | The release keystore, base64-encoded. |
| `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD` | What opens it. Without these the APK is signed with the debug key and the log says so. |

Nothing else. Every application repository is public, so the job's own token checks each of them out along
with the `base/` submodule that pins its core.

While they were private that was impossible — a job's token sees only the repository it runs in, and a
submodule fetch reaches for that same token — and three read-only deploy keys stood in for it. They were
removed the day the repositories went public. If any of them ever goes private again, the keys come back:
one ed25519 pair per repository, public half added as a read-only deploy key, private half stored here as
a secret, and each checkout given `ssh-key:` and an explicit second checkout of the core at
`git rev-parse HEAD:base`. Deploy keys also have to be enabled for the organisation, which they are not by
default.

## Updating

Noctorium asks GitHub for `/releases/latest` of this repository once at launch and offers what it finds.
That endpoint **ignores drafts and pre-releases**, so nothing is offered to anybody until a draft is
published. Downloads are checked against `SHA256SUMS.txt` from the same release before anything is
installed; a release without one is refused with a link to the page instead.

| Installed as | What happens |
| --- | --- |
| Windows installer | Downloads the `.exe`, checks it, runs it, and closes so its files can be replaced |
| `.deb` / `.rpm` | Downloads the package and hands it to the package manager through a `pkexec` prompt |
| Arch package, AppImage, Flatpak | Told there is an update, and sent to the release page — or run the installer again |
| Android | Downloads the APK and hands it to Android's package installer, which asks again |
| Unzipped folder, or Gradle | Told there is an update, and sent to the release page |

## The three shapes of the version

The tag is the source, but it reaches three places that disagree about what a version may look like.

| Where | From `v1.2.3` | Rule it has to satisfy |
| --- | --- | --- |
| Gradle, and the release page | `1.2.3` | anything |
| `.msi`, `.deb`, `.rpm` | `1.2.3` | rpm refuses a hyphen; msi wants three numeric parts |
| Arch package | `1.2.3` | pacman refuses a hyphen as well, so `v1.2.3-beta.1` becomes `1.2.3beta.1`, which pacman orders before `1.2.3` |
| Android `versionCode` | `10203` | an integer that increases with every release |

So `v1.2.3-beta.1` is a perfectly good tag: the release says `1.2.3-beta.1`, the packages are built as
`1.2.3`, and Android gets `10203`. `versionCode` packs the parts as `major * 10000 + minor * 100 + patch`.

## Notes from earlier releases

`notes/` keeps the release notes of every version so far, including the ones published under the
application's previous name, Spiceity.
