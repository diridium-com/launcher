# Releasing

Releases are cut by pushing a version tag. The CI workflow
(`.github/workflows/build-launcher.yml`) builds all platforms on every push, with
a read-only token. Only when a `v*` tag is pushed does a separate `release` job,
the only one that can write, create a **draft** GitHub release from the checked
build files. Merging to `master` does not release.

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
   whose notes are a table of those files. Review the files on the GitHub
   Releases page, write the release notes above the table, then **Publish** it.

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
