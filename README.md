# Noctorium releases

This is where Noctorium is published: the Windows installer, the Debian and Fedora packages and the
Android APK, with a `SHA256SUMS.txt` beside them. The applications check here for updates. The player
itself is elsewhere — the desktop is [Noctorium-Desktop](https://github.com/Noctorium/Noctorium-Desktop),
the phone is [Noctorium-Mobile](https://github.com/Noctorium/Noctorium-Mobile), and what they share is
[Noctorium-Base](https://github.com/Noctorium/Noctorium-Base).

What *is* here, besides the release pipeline, is the pair of small installers.

## The installers

Two programs that do one thing: ask GitHub what the latest release is, fetch the file for the machine
they are on, check it against the checksum published beside it, and hand it to whatever installs software
there. They are what somebody downloads once. Everything after that is the application's own updater.

| | Where | Built from |
| --- | --- | --- |
| `Noctorium-Installer-windows-x64.exe`, `noctorium-installer-linux-x64` | Windows, Debian and Fedora | `pc/`, in Rust |
| `Noctorium-Installer-android.apk` | Android | `phone/`, in Dart with Flutter |

Neither installs anything itself. The PC one runs the Windows installer, or hands the package to `apt` or
`dnf` through `pkexec` so dependencies are resolved rather than merely reported. The phone one hands the
APK to Android's own package installer, which shows its own screen and asks again — and the first time
sends you to a settings page to allow it at all.

Both refuse to install anything they cannot check. A release with no `SHA256SUMS.txt`, or a download that
does not match the checksum published for it, is deleted rather than run. Both skip their own files in a
release, so the installer never offers to install itself.

Both read `GITHUB_TOKEN` from the environment if it is set, which is the only way either of them can see a
release while this repository is private.

```bash
cd pc    && cargo test && cargo build --release    # the PC installer
cd phone && flutter test && flutter build apk      # the phone installer
```

## Which file

| Platform | File |
| --- | --- |
| Windows | `Noctorium-<version>-windows-x64-setup.exe`, or the `.msi` for deployment |
| Debian, Ubuntu, Mint | `noctorium_<version>_amd64.deb` |
| Fedora, RHEL, openSUSE | `noctorium-<version>.x86_64.rpm` |
| Android | `Noctorium-<version>.apk` |

The desktop packages carry their own Java runtime and their own Chromium for the sign-in window; the
Windows ones carry mpv and yt-dlp as well. Nothing needs installing first. **Close Noctorium before
installing over it** — the installer cannot replace files the running program holds open.

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

```bash
git tag v1.2.3
git push origin v1.2.3
```

That is the whole of it. `.github/workflows/release.yml` checks out Noctorium-Desktop and Noctorium-Mobile
at `main`, each with the Noctorium-Base commit it pins, runs the tests, packages each platform on its own
runner, and opens a **draft** release with everything attached and the checksums beside it. It is a draft
on purpose — read it, check the files are the sizes you expect, write the notes, and publish it yourself.

To build without releasing, run the workflow by hand from the Actions tab and give it a version, and
optionally a branch or commit of each application. The artifacts are attached to the run for two weeks
and no release is created.

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
published — and it answers 404 for a private repository, so the updater is dormant until this repository
is public. Downloads are checked against `SHA256SUMS.txt` from the same release before anything is
installed; a release without one is refused with a link to the page instead.

| Installed as | What happens |
| --- | --- |
| Windows installer | Downloads the `.exe`, checks it, runs it, and closes so its files can be replaced |
| `.deb` / `.rpm` | Downloads the package and hands it to the package manager through a `pkexec` prompt |
| Android | Downloads the APK and hands it to Android's package installer, which asks again |
| Unzipped folder, or Gradle | Told there is an update, and sent to the release page |

## The three shapes of the version

The tag is the source, but it reaches three places that disagree about what a version may look like.

| Where | From `v1.2.3` | Rule it has to satisfy |
| --- | --- | --- |
| Gradle, and the release page | `1.2.3` | anything |
| `.msi`, `.deb`, `.rpm` | `1.2.3` | rpm refuses a hyphen; msi wants three numeric parts |
| Android `versionCode` | `10203` | an integer that increases with every release |

So `v1.2.3-beta.1` is a perfectly good tag: the release says `1.2.3-beta.1`, the packages are built as
`1.2.3`, and Android gets `10203`. `versionCode` packs the parts as `major * 10000 + minor * 100 + patch`.

## Notes from earlier releases

`notes/` keeps the release notes of every version so far, including the ones published under the
application's previous name, Spiceity.
