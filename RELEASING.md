# Releasing

Releases are cut by pushing a version tag. The CI workflow
(`.github/workflows/build-launcher.yml`) builds all platforms and creates a
**draft** GitHub release only when a `v*` tag is pushed. Regular branch and PR
pushes only build; merging to `master` does not release.

## Steps

1. **Bump the version** in both `package.json` and `src-tauri/Cargo.toml` to
   `X.Y.Z`. Keep them in sync (`tauri.conf.json` has no version field and defers
   to `package.json`; the About box reads the Cargo version). Refresh the
   lockfiles:

   ```
   npm install                       # updates package-lock.json
   ( cd src-tauri && cargo check )   # updates Cargo.lock
   ```

2. Open a PR with the bump, let CI go green, and merge to `master`.

3. **Tag the merged commit and push the tag:**

   ```
   git checkout master && git pull
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```

   The tag must equal `v` + the manifest version, or the release job fails its
   `Verify tag matches manifest version` guard (this prevents mis-tagging and
   clobbering an existing release).

4. CI builds every platform and creates a **draft** release `vX.Y.Z`. Review its
   assets and notes on the GitHub Releases page, then **Publish** it.

## Notes

- The release is a draft on purpose, so a human reviews it before it goes
  public. Nothing is published automatically.
- Because releases fire only on a `v*` tag, later merges to `master` will not
  re-upload over an already-published release.
