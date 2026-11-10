# Releasing

A release starts with a pushed version tag. The CI workflow
(`.github/workflows/build-launcher.yml`) builds every platform on every push, with
a read-only token. When the push is a `v*` tag, its `release` job, the only job in
that workflow that can write, creates a **draft** GitHub release from the checked
build files. Merging to `master` never releases anything.

The Windows files are signed before the draft is published (steps 6 to 9, and
"Windows signing" below). Each of the two Windows workflows you run by hand in those
steps has one job with write access, since only a write token can see a draft. That
job only downloads the draft's files.

## Steps

1. **Bump the version** to `X.Y.Z` in `package.json`, in both root `version`
   fields of `package-lock.json`, and in `src-tauri/Cargo.toml`, then refresh
   `Cargo.lock`:

   ```
   ( cd src-tauri && cargo check )   # updates the launcher's entry in Cargo.lock
   ```

   The versions must match. `tauri.conf.json` has no version field and takes it from
   `package.json`, and the About box reads the Cargo version. Edit the two version
   fields in `package-lock.json` by hand: an npm older than the one that wrote the
   file would rewrite all of it.

2. **Open a PR** with the bump, let CI go green, and merge to `master`.

3. **Check dependency ages.** Every dependency version that is new since the
   last release must be at least 14 days old. The exception is a version that
   fixes a critical or high advisory in the app as shipped. In that case the
   release notes say so.

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

   The `guard` job fails unless the tag is `v` plus the version in every file
   from step 1. It also refuses a tag whose release is already published.

5. **CI builds every platform** and checks that the builds produced exactly the
   12 files a release takes from CI (the MSIX packages come from step 7). The
   `release` job then creates the draft release `vX.Y.Z` with the 8 macOS and
   Linux files; the Windows files are signed first (steps 6 to 9). The Windows
   runners also upload `sign-inputs-*`, three files per architecture:
   - two copies of `Launcher.exe`, one marked for each installer type, each
     checked against the exe inside that run's own unsigned installer;
   - the NSIS uninstaller, checked against the `uninstall.exe` inside that run's
     setup.exe.

6. **Sign the `Launcher.exe` copies and the uninstallers.** Run this in the
   owner's remotekey session, from the repo root, with `yubikey-pkcs11.cfg` and
   `certchain.pem` linked in (both are gitignored) and `JSIGN_JAR` set to the
   jsign 7.6 jar, whose SHA-256 the script checks:

   ```
   RUN=<run id of the tag's build>
   scripts/ci-hashes.sh "$RUN" vX.Y.Z > sign-inputs.sha256
   gh run download "$RUN" -p 'sign-inputs-*' -D sign-inputs
   scripts/sign-windows.sh sign-inputs.sha256 signed-exes sign-inputs/*/*.exe
   gh release upload vX.Y.Z signed-exes/*.exe
   ```

   `ci-hashes.sh` refuses a run that isn't the tag's successful build.
   `sign-windows.sh` then signs nothing unless every file's SHA-256 matches the
   run's log, and asks for the PIN once. It checks that each signed copy:
   - has a DigiCert timestamp;
   - is signed by Diridium Technologies, Inc. through SSL.com's intermediate;
   - differs from the original only by the signature.

   It also verifies each signed `.exe` and `.msi` with osslsigncode. An MSIX's
   signature is read but not verified here; step 9 has Windows verify it. The
   unsigned copies it kept are in `ci-originals/`, named `<file>.unsigned`.

   It works in `signed-exes.partial` and renames that to `signed-exes` only when
   every check passes, so after a failure the upload line finds nothing. Use a
   new folder name for the next attempt. The six files uploaded here stay in the
   draft only until step 8.

7. **Build the Windows installers and MSIX packages from the signed files:**

   ```
   gh workflow run windows-installers.yml --ref vX.Y.Z -f tag=vX.Y.Z
   ```

   It builds each installer with `tauri bundle --no-binary-patching`, so the
   signed exe goes in unchanged. While makensis builds setup.exe, the signed
   uninstaller replaces the one makensis built, but only if the signed copy is
   that uninstaller plus a signature. `scripts/pack-msix.ps1` packs the MSIX
   around the signed NSIS-marked exe. The workflow checks the signed files inside
   each result and uploads all six as `windows-installers-*`. `--ref vX.Y.Z` runs
   the workflow file from the tag, so it matches the scripts it checks out.

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

   It first checks that the draft has exactly the 14 release files. Then, on a
   fresh runner for each architecture and file type (setup.exe, MSI, MSIX), it:
   - checks the file's signature;
   - installs it silently;
   - checks that the installed `Launcher.exe`, and setup.exe's `uninstall.exe`,
     are signed and identical to the copies inside the file;
   - starts Launcher, then uninstalls.

   Every signature must be Valid without any extra trusted certificate.

10. **Review the draft.** It should have exactly the 14 release files and no
    temporary exes. Write the release notes above the file table, then publish
    it.

## Notes

- Nothing is published automatically: a person reviews the draft and publishes
  it.
- Re-running a tag's workflow refills its draft. A published release's files are
  never replaced.
- An unpublished draft that is wrong can be thrown away, since nobody has it yet.
  Delete the draft and the tag, fix the problem, and tag the same version again.
  Workflow artifacts that contain signed files are kept for one day, so a
  thrown-away build stops being downloadable after that. Only the repository
  owner can create, move or delete `v*` tags. A clone that already fetched the
  old tag needs `git fetch --tags --force`.

## Windows signing

- GitHub Actions builds every file a release ships, from the tag. Signing adds
  an Authenticode signature to those files and changes nothing else.
  `scripts/compare-signed.py`
  checks that for every signed file: byte for byte for the exes, and by content
  for the MSI (every stream) and the MSIX (every entry and content type). The key
  is a hardware key on the owner's machine, forwarded into the dev VM for the
  length of a signing step. It is never in CI, the signing steps build nothing,
  and every signature needs the PIN.
- A release takes two CI runs because Tauri writes the installer type into
  `Launcher.exe` while it builds each installer (tauri-bundler, `bundle.rs`,
  `patch_binary`), so the exe must be signed after that mark and before the
  installer is built around it.
  `scripts/patch-bundle-type.py` makes the same mark Tauri makes, and CI checks
  its output byte for byte against the exe inside Tauri's own installers.
- The uninstaller that setup.exe installs (`uninstall.exe`) is built by makensis
  and exists as a file only while makensis runs. makensis hands it to Tauri's
  signing command, which in CI is `scripts/sign-command.ps1`. That script signs
  nothing. In the tag's build it keeps a copy of the uninstaller for step 6 to
  sign, and in `windows-installers.yml` it swaps the signed copy in. makensis builds the same
  uninstaller in both runs and on both architectures (measured 2026-11-08 with
  tauri-cli 2.12.4 and makensis 3.13), and the swap refuses a signed copy of any
  other uninstaller.
- The NSIS plugin DLLs that setup.exe and the uninstaller unpack while they run
  are not signed. The uninstaller carries copies of two of them, so signing them
  would take a third CI run and signing session. Tauri also never hands one of
  them, LangDLL.dll, to the signing command. `Launcher.exe`, the uninstaller and
  all six Windows release files are signed.
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
  `installers/`, `signed-installers/`, the `.partial` folders, retry folders
  such as `signed-exes-2/`, and the two `.sha256` lists) are gitignored.

## Upgrade test

Every build's `upgrade` job installs the latest published release of
diridium-com/launcher on fresh x64 and ARM64 Windows runners, then the build's
own installer over it.

- With the same installer type (setup.exe over setup.exe, MSI over MSI), the
  build fails unless exactly one Launcher is left, at the new version and
  publisher, with the saved connections untouched and the app starting.
- Switching types may leave both versions installed, which the job reports as
  warnings. Tauri's MSI never removes a setup.exe install, and setup.exe removes
  an MSI install only from the same publisher, after a dialog the test can't
  answer. Apart from that dialog, the build fails unless the switch installs the
  new version and starts it, with the connections untouched.

The release notes tell users to upgrade with the installer type they used, or
to uninstall first to switch.

## Real-launch test

Every build's `launch` job installs the build's Linux x64 `.deb` on an Ubuntu
runner, starts OIE 4.7.0 from its release tarball, and launches a new connection
through Launcher (`scripts/e2e/real-launch.py`). The build fails unless the
certificate prompt appears, the status bar shows progress after it, the launch
finishes, and the engine's event log shows the administrator logging in as admin.
The tag's release job waits for this job and the upgrade job.
