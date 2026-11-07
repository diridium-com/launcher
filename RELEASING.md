# Releasing

Releases are cut by pushing a version tag. The CI workflow
(`.github/workflows/build-launcher.yml`) builds all platforms on every push, with
a read-only token. Only when a `v*` tag is pushed does a separate `release` job,
the only one that can write, create a **draft** GitHub release from the checked
build files. Merging to `master` does not release. The Windows files are signed
before the draft is published (steps 6 to 9, and "Windows signing" below).

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
   pinned GitHub actions and the CI Node version. It exits non-zero when
   anything is too young or couldn't be checked, and prints the date from which
   everything is old enough.

4. **Tag the merged commit and push the tag:**

   ```
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

   The tag must equal `v` + the version in every file from step 1, or the
   `guard` job fails, which also refuses a tag whose release is already
   published.

5. CI builds every platform, checks that the builds produced exactly the files a
   release carries, and the `release` job creates a **draft** release `vX.Y.Z`
   with the 8 macOS and Linux files. The Windows files are signed first (steps 6
   to 9). The Windows runners also upload `sign-inputs-*`: two copies of
   `Launcher.exe` per architecture, one marked for each installer type, each
   checked against the exe inside that run's own unsigned installer.

6. **Sign the `Launcher.exe` copies.** In the owner's remotekey session, from the
   repo root, with `yubikey-pkcs11.cfg` and `certchain.pem` linked in (both
   gitignored) and `JSIGN_JAR` pointing at a jsign jar with a `.sha256` beside it:

   ```
   RUN=<run id of the tag's build>
   scripts/ci-hashes.sh "$RUN" > sign-inputs.sha256
   gh run download "$RUN" -p 'sign-inputs-*' -D sign-inputs
   scripts/sign-windows.sh sign-inputs.sha256 signed-exes sign-inputs/*/*.exe
   gh release upload vX.Y.Z signed-exes/*.exe
   ```

   `sign-windows.sh` signs nothing unless every file's SHA-256 matches the run's
   log, asks for the PIN once, and then checks each signed copy: it verifies,
   carries a DigiCert timestamp and SSL.com's intermediate, is signed by
   Diridium Technologies, Inc., and is the original plus a signature and nothing
   else. The four exes stay in the draft only until step 8.

7. **Build the Windows installers from the signed exes:**

   ```
   gh workflow run windows-installers.yml -f tag=vX.Y.Z
   ```

   It builds each installer with `tauri bundle --no-binary-patching`, so the
   signed exe goes in unchanged, checks the exe inside each installer is the
   signed one, and uploads the installers as `windows-installers-*`.

8. **Sign the installers**, in a remotekey session as in step 6:

   ```
   RUN2=<run id of windows-installers.yml>
   scripts/ci-hashes.sh "$RUN2" > installers.sha256
   gh run download "$RUN2" -p 'windows-installers-*' -D installers
   scripts/check-installers.sh installers signed-exes
   scripts/sign-windows.sh installers.sha256 signed-installers installers/*/*
   gh release upload vX.Y.Z signed-installers/*.exe signed-installers/*.msi
   for a in x64 arm64; do for k in nsis msi; do
     gh release delete-asset vX.Y.Z "Launcher_X.Y.Z_${a}-${k}.exe" -y
   done; done
   ```

9. **Test the draft's Windows installers:**

   ```
   gh workflow run test-windows-installers.yml -f tag=vX.Y.Z
   ```

   On a fresh runner for each architecture and installer type, it checks the
   installer's signature, installs it silently, checks the installed
   `Launcher.exe` is signed and identical to the one inside the installer,
   starts it, and uninstalls. Every signature must be Valid on a stock Windows.

10. Review the draft: exactly the 12 release files, no temporary exes. Write the
    release notes above the file table, then **Publish** it.

## Notes

- The release is a draft on purpose, so a human reviews it before it goes
  public. Nothing is published automatically.
- Releases fire only on a `v*` tag, so later merges to `master` never touch a
  release. Re-running a tag's workflow refills its draft, and a published
  release's files are never replaced.
- A draft that's wrong can be thrown away while it's unpublished, because nobody
  has it yet: delete the draft and the tag, fix, and tag the same version again.
  Only the repository owner can create, move or delete `v*` tags. A clone that
  already fetched the old tag needs `git fetch --tags --force`.

## Windows signing

- Every file a release ships is built by GitHub Actions from the tag. Signing
  adds an Authenticode signature to what CI built and changes nothing else, and
  `scripts/compare-signed.py` checks exactly that for every signed file. The key
  is a hardware key on the owner's machine, used over a forwarded session for
  the length of a signing step. It is never in CI, and no build runs where it is.
- Why two CI runs: Tauri writes the installer type into `Launcher.exe` while it
  builds each installer (tauri-bundler, `bundle.rs`, `patch_binary`), so the exe
  must be signed after that mark and before the installer is built around it.
  `scripts/patch-bundle-type.py` makes the same mark Tauri makes, and CI checks
  its output byte for byte against the exe inside Tauri's own installers.
- Not signed: setup.exe's uninstaller (`uninstall.exe`) and the NSIS plugin DLLs.
  They exist only inside CI's installer build, and signing them would need the
  key there. `Launcher.exe` and all four installers are signed.
- jsign follows the same 14-day rule as every other dependency.
- Re-running the tag's build after step 6 refuses to refill the draft while
  signed Windows files from the earlier run are in it. Delete them and sign the
  new run's files again.
- macOS and Linux files are not signed. Their SHA-256s are on the release page.
