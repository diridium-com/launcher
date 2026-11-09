# Design notes

This file explains why parts of Launcher work the way they do, for anyone reading or changing
the code. It was written for 2.4.1 on 2026-11-09. Later changes add to it.

## Certificate pinning

The first time a connection reaches an engine, Launcher shows the SHA-256 of the engine's TLS
certificate and asks whether to trust it. On later launches a different certificate stops the
launch until the operator accepts the new one. The JNLP and every jar then come from that
pinned server.

Launcher doesn't check jar signatures. A signature check would only matter if the engine
server were compromised, and an attacker with the engine already has its messages, channels
and connector credentials. Refusing to launch would not undo any of that. A prompt asking
whether to trust a new signer would be one the operator can't judge, and it would appear on
every rebuild and certificate change. A signature check needs a signer known in advance to be
trusted, and the engines Launcher works with (OIE and the engines derived from the same code)
don't share one.

A freshly downloaded jar isn't compared with the hash in the JNLP. Launcher writes the
download to a temporary file and renames it into place. On the next launch the hash decides
whether the cached jar is current, so a damaged download is replaced then. Comparing at
download time would add no security, because whoever can change the jars can also change the
hashes in the JNLP that lists them.

## Addresses and the JNLP

Launcher fetches the engine's JNLP from the address in the connection, and fetches every jar
from that same address. It ignores the JNLP's `codebase` attribute: the connection's address
is the one Launcher has just fetched the JNLP from, while `codebase` is the address the engine
advertises. On OIE 4.6.0 behind a port mapping, `codebase` followed the address actually used;
the comment above the jar loop in `src-tauri/src/webstart.rs` has the details.

The administrator logs in to the server URL in the JNLP's arguments, passed on as the engine
wrote it. An engine behind a port mapping can advertise a port the operator can't reach: OIE
4.6.0 behind a Docker mapping of 8444 to 8443 put 8443 in its JNLP, and only 8444 worked. When
the two ports differ, Launcher stops and asks before going on. It compares only ports. Host
names often differ in setups that work, such as SSH tunnels and port forwards.

## The jar cache

Connections to the same engine type and version share one jar cache. A connection set not to
cache gets its own folder instead, emptied at the start of each launch. Every launch fetches
the JNLP again and checks each cached jar against the hash the JNLP lists. A core jar that
differs belongs to another engine claiming the same version, so Launcher asks before
replacing it. An extension jar that differs was upgraded on the server and is downloaded again
without asking.

Launcher keeps no parsed JNLP results between launches, so every launch runs the jar check.
Version 2.3.0 removed an in-memory cache that skipped the check for two minutes after each
launch. Launching again that soon usually follows an engine upgrade or an extension install,
which is when the check matters most. The cache saved a few seconds per repeat launch.

## Saved passwords

A connection's username and password are optional. When given, they are stored in plain text
in `~/.launcher/launcher-data.json`. On macOS and Linux, Launcher creates that file readable by
its owner only and tightens its permissions again on every start. The password reaches the
administrator on its command line, so it shows in the process list while the administrator
runs. The connection form warns about both next to the password field.

## Windows and WebView2

Launcher's window is a Tauri webview, which on Windows is Microsoft Edge WebView2. The
installers download WebView2 when it is missing, and the README explains how to install it
first on a machine without internet access. Bundling WebView2 would add about 127 MB to every
Windows download, though only a minority of machines lack it. A fixed-version runtime, a pinned
copy of WebView2 shipped with the app, would add more, and the project would then have to keep
that browser engine up to date. The MSIX package
can't install WebView2, which the README also says.

## Release builds and signing

GitHub Actions builds every release file from the release tag. The Windows files are signed
afterwards with a hardware key that never enters CI, and only files whose SHA-256 matches the
CI run's log are signed. Tauri writes the installer type into `Launcher.exe` while building
each installer, so the exe has to be signed before the installers are built around it. A
release therefore takes two CI runs and two signing steps. RELEASING.md has the steps and the
details, including how the NSIS uninstaller gets signed.

## Linux builds

The Linux packages are built on GitHub's `ubuntu-latest` runner (Ubuntu 26.04 at the time of
writing) and its `ubuntu-24.04-arm` runner. They need a C library at least as new as the one
they were built with. Measured on 2026-11-09: the x64 `.deb` binary needs glibc 2.39, the x64
AppImage needs 2.43 for the libraries it bundles, and the ARM64 AppImage needs 2.39. None of
them start on Ubuntu 22.04, which has glibc 2.35, while a build made on 22.04 runs there.

## Expected warnings

- CI's npm audit step prints `allowed:` lines for advisories in packages used only to build
  the frontend. `build-launcher.yml` lists them, with the date each was checked.
- The upgrade job's two switching cases (a 2.4.0 MSI upgraded with setup.exe, and the
  reverse) report warnings when both versions stay installed. Tauri's installers behave that
  way, and the release notes tell users to upgrade with the installer type they used.
- `scripts/dependency-ages.py` warns that `dtolnay/rust-toolchain` has no GitHub release for
  its `stable` pin, and dates it by its commit instead.
- On a Linux VM without hardware OpenGL, Launcher prints
  `libEGL warning: DRI2: failed to authenticate` at start. The app still works.

Clippy runs with warnings as errors, and `oxlint` fails the build on any frontend warning, so
neither should print warnings.

## What the tests don't cover

- There are no frontend unit tests. The real launch in CI (`scripts/e2e/real-launch.py`)
  covers one path through the frontend: a first launch with the certificate prompt.
- The real launch runs on Linux x64 against OIE 4.7.0 only. Windows and macOS launches, and
  other engine versions, are checked by hand.
- No automated test drives the certificate-changed, port-mismatch or cache-mismatch prompts.
- The Windows installers are installed and started on GitHub's runners only while a release
  is being prepared (`test-windows-installers.yml`). Every build runs the upgrade test from
  the latest published release.
- The Linux packages are not installed on other distributions.
