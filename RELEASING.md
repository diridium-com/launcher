# Releasing

Releases are cut by pushing a version tag. The CI workflow
(`.github/workflows/build-launcher.yml`) builds all platforms on every push, with
a read-only token. Only when a `v*` tag is pushed does a separate `release` job,
the only job in that workflow that can write, create a **draft** GitHub release
from the checked build files. Merging to `master` does not release. The Windows
files are signed before the draft is published (steps 6 to 9, and "Windows
signing" below). The two Windows workflows run by hand in those steps each have
one job that can write, because only such a token sees a draft; it only downloads
the draft's files and builds nothing.

## Steps

1. **Bump the version** to `X.Y.Z` in `package.json`, in both root `version`
   fields of `package-lock.json`, and in `src-tauri/Cargo.toml`, then refresh
   `Cargo.lock`:

   ```
   ( cd src-tauri && cargo check )   # updates the launcher's entry in Cargo.lock
   ```

   Keep them in sync: `tauri.conf.json` has no version field and takes it from
   `package.json`, and the About box reads the Cargo version. Editing
   `package-lock.json`'s two version fields by hand keeps an npm older than the
   one that wrote the file from rewriting all of it.

2. Open a PR with the bump, let CI go green, and merge to `master`.

3. **Check dependency ages.** Every dependency version that is new since the
   last release must be at least 14 days old, unless it fixes a critical or high
   advisory in the app as shipped, which the release notes then say:

   ```
   git checkout master && git pull
   python3 scripts/dependency-ages.py
   ```

   It compares the latest `v*` tag with `HEAD`: npm packages, Rust crates,
   pinned GitHub actions (dated by their GitHub release), the CI Node and Rust
   versions, jsign and the NuGet build tools. The NSIS and WiX the Tauri CLI
   downloads come with the CLI's version, which the npm check covers. It exits
   non-zero when anything is too young or couldn't be checked, and prints the
   date from which everything is old enough.

4. **Tag the merged commit and push the tag:**

   ```
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

   The tag must equal `v` + the version in every file from step 1, or the
   `guard` job fails, which also refuses a tag whose release is already
   published.

5. CI builds every platform, checks that the builds produced exactly the 12
   files a release gets from them (the MSIX packages come from step 7), and the
   `release` job creates a **draft** release `vX.Y.Z` with the 8 macOS and Linux
   files. The Windows files are signed first (steps 6 to 9). The Windows runners
   also upload `sign-inputs-*`, three files per architecture: two copies of
   `Launcher.exe`, one marked for each installer type, each checked against the
   exe inside that run's own unsigned installer, and the NSIS uninstaller,
   checked against the `uninstall.exe` inside that run's setup.exe.

6. **Sign the `Launcher.exe` copies and the uninstallers.** In the owner's
   remotekey session, from the repo root, with `yubikey-pkcs11.cfg` and
   `certchain.pem` linked in (both gitignored) and `JSIGN_JAR` pointing at the
   jsign 7.6 jar (the script checks its SHA-256):

   ```
   RUN=<run id of the tag's build>
   scripts/ci-hashes.sh "$RUN" vX.Y.Z > sign-inputs.sha256
   gh run download "$RUN" -p 'sign-inputs-*' -D sign-inputs
   scripts/sign-windows.sh sign-inputs.sha256 signed-exes sign-inputs/*/*.exe
   gh release upload vX.Y.Z signed-exes/*.exe
   ```

   `ci-hashes.sh` refuses a run that isn't the tag's successful build. Then
   `sign-windows.sh` signs nothing unless every file's SHA-256 matches the run's
   log, asks for the PIN once, and checks each signed copy: it verifies, carries
   a DigiCert timestamp, is signed by Diridium Technologies, Inc. through
   SSL.com's intermediate, and is the original plus a signature and nothing
   else. It works in `signed-exes.partial` and renames it to `signed-exes` only
   when every check passes, so after a failure the upload line finds nothing;
   pick a new folder name for the next attempt. The six files stay in the draft
   only until step 8.

7. **Build the Windows installers and MSIX packages from the signed files:**

   ```
   gh workflow run windows-installers.yml --ref vX.Y.Z -f tag=vX.Y.Z
   ```

   It builds each installer with `tauri bundle --no-binary-patching`, so the
   signed exe goes in unchanged. While makensis builds setup.exe, the signed
   uninstaller replaces the one it built, which is accepted only if the signed
   copy is that uninstaller plus a signature. `scripts/pack-msix.ps1` packs the
   MSIX around the signed NSIS-marked exe. The workflow checks the signed files
   inside each result and uploads all six as `windows-installers-*`. `--ref`
   runs the tag's own copy of the workflow, matching the scripts it checks out.

8. **Sign the installers and packages**, in a remotekey session as in step 6:

   ```
   RUN2=<run id of windows-installers.yml>
   scripts/ci-hashes.sh "$RUN2" vX.Y.Z > installers.sha256
   gh run download "$RUN2" -p 'windows-installers-*' -D installers
   scripts/check-installers.sh installers signed-exes
   scripts/sign-windows.sh installers.sha256 signed-installers installers/*/*
   gh release upload vX.Y.Z signed-installers/*.exe signed-installers/*.msi signed-installers/*.msix
   ```

   Then take step 6's files out of the draft:

   ```
   gh release delete-asset vX.Y.Z Launcher_X.Y.Z_x64-nsis.exe -y
   gh release delete-asset vX.Y.Z Launcher_X.Y.Z_x64-msi.exe -y
   gh release delete-asset vX.Y.Z Launcher_X.Y.Z_x64-uninstall.exe -y
   gh release delete-asset vX.Y.Z Launcher_X.Y.Z_arm64-nsis.exe -y
   gh release delete-asset vX.Y.Z Launcher_X.Y.Z_arm64-msi.exe -y
   gh release delete-asset vX.Y.Z Launcher_X.Y.Z_arm64-uninstall.exe -y
   ```

9. **Test the draft's Windows installers:**

   ```
   gh workflow run test-windows-installers.yml --ref vX.Y.Z -f tag=vX.Y.Z
   ```

   It first checks the draft holds exactly the 14 release files. Then, on a
   fresh runner for each architecture and file type (setup.exe, MSI, MSIX), it
   checks the file's signature, installs it silently, checks the
   installed `Launcher.exe` (and setup.exe's `uninstall.exe`) is signed and
   identical to the one inside it, starts it, and uninstalls. Every signature
   must be Valid on a stock Windows.

10. Review the draft: exactly the 14 release files, no temporary exes. Write the
    release notes above the file table, then **Publish** it.

## Notes

- The release is a draft on purpose, so a human reviews it before it goes
  public. Nothing is published automatically.
- Releases fire only on a `v*` tag, so later merges to `master` never touch a
  release. Re-running a tag's workflow refills its draft, and a published
  release's files are never replaced.
- A draft that's wrong can be thrown away while it's unpublished, because nobody
  has it yet: delete the draft and the tag, fix, and tag the same version again.
  The workflow artifacts that hold signed files are kept for one day, so they
  don't keep a thrown-away build downloadable.
  Only the repository owner can create, move or delete `v*` tags. A clone that
  already fetched the old tag needs `git fetch --tags --force`.

## Windows signing

- Every file a release ships is built by GitHub Actions from the tag. Signing
  adds an Authenticode signature to what CI built and changes nothing else.
  `scripts/compare-signed.py` checks that for every signed file: byte for byte
  for the exes, and by content for the MSI (every stream) and the MSIX (every
  entry and content type). The key is a hardware key on the owner's machine,
  forwarded into the dev VM for the length of a signing step. It is never in CI;
  the signing steps build nothing, and every signature needs the PIN.
- Why two CI runs: Tauri writes the installer type into `Launcher.exe` while it
  builds each installer (tauri-bundler, `bundle.rs`, `patch_binary`), so the exe
  must be signed after that mark and before the installer is built around it.
  `scripts/patch-bundle-type.py` makes the same mark Tauri makes, and CI checks
  its output byte for byte against the exe inside Tauri's own installers.
- The uninstaller setup.exe installs (`uninstall.exe`) exists as a file only
  while makensis runs, which hands it to Tauri's signing command. In CI that
  command is `scripts/sign-command.ps1`, which never signs: in the tag's build
  it keeps a copy, which step 6 signs, and in `windows-installers.yml` it
  swaps the signed copy in. makensis builds the same uninstaller in both runs
  and on both architectures (measured 2026-11-08 with tauri-cli 2.12.4 and
  makensis 3.13), and the swap refuses a signed copy of any other uninstaller.
- Not signed: the NSIS plugin DLLs that setup.exe and the uninstaller unpack
  while they run. The uninstaller carries copies of two of them, so signing
  them would take a third CI run and signing session; and Tauri never hands one
  of them, LangDLL.dll, to the signing command. `Launcher.exe`, the
  uninstaller and all six Windows release files are signed.
- The MSIX is packed by `scripts/pack-msix.ps1` with makeappx from Microsoft's
  NuGet package Microsoft.Windows.SDK.BuildTools, checked against NuGet's
  SHA-512. The manifest is `src-tauri/msix/AppxManifest.xml`. Its Publisher
  must match the signing certificate's subject, so a dry run with a test
  certificate passes that subject as `msix_publisher`.
- jsign is pinned by version and SHA-256 in `scripts/sign-windows.sh`, and
  `scripts/dependency-ages.py` checks its age like any other dependency.
- Re-running the tag's build after step 6 refuses to refill the draft while
  signed Windows files from the earlier run are in it. Delete them and sign the
  new run's files again.
- macOS and Linux files are not signed. Their SHA-256s are on the release page.
- The signing steps' working folders (`sign-inputs/`, `signed-exes/`,
  `installers/`, `signed-installers/`, the `.partial` folders and the two
  `.sha256` lists) are gitignored.

## Upgrade test

Every build (`upgrade` job) installs the latest published release of
diridium-com/launcher on fresh x64 and ARM64 Windows runners, then the build's
own installer over it. Upgrading with the same installer type (setup.exe over
setup.exe, MSI over MSI) must leave one Launcher at the new version and
publisher, the saved connections untouched, and the app starting, or the build
fails. Switching types may leave both versions installed, which is reported as
warnings: Tauri's MSI never removes a setup.exe install, and setup.exe removes
an MSI install only from the same publisher, after a dialog the test can't
answer. Apart from that dialog, a switch must still install the new version and
start it, with the connections untouched, or the build fails. Release notes tell
users to upgrade with the type they installed, or uninstall first to switch.
