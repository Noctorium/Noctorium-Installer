# Noctorium releases

This is where Noctorium is published: the Windows installer, the Debian and Fedora packages and the
Android APK, with a `SHA256SUMS.txt` beside them. The applications check here for updates. There is no
application code in this repository — the desktop is
[Noctorium-Desktop](https://github.com/Noctorium/Noctorium-Desktop), the phone is
[Noctorium-Mobile](https://github.com/Noctorium/Noctorium-Mobile), and what they share is
[Noctorium-Base](https://github.com/Noctorium/Noctorium-Base).

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
| `NOCTORIUM_CHECKOUT_TOKEN` | A fine-grained personal access token with **Contents: read** on Noctorium-Base, Noctorium-Desktop and Noctorium-Mobile. Needed only while those repositories are private: the job's own token can see nothing but this repository. |
| `ANDROID_KEYSTORE_BASE64` | The release keystore, base64-encoded. |
| `ANDROID_KEYSTORE_PASSWORD`, `ANDROID_KEY_ALIAS`, `ANDROID_KEY_PASSWORD` | What opens it. Without these the APK is signed with the debug key and the log says so. |

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
