# Homebrew Formula

This directory contains the Homebrew formula template for `aes67-tools`.

The normal release path is the manual GitHub Actions workflow
`.github/workflows/release-run.yml`. For a non-dry-run release, leave
`update_homebrew=true`; the workflow uploads the GitHub release assets and then
commits the generated formula to the live tap repository
`wiccy46/homebrew-aes67`.

That cross-repository push requires a one-time Actions secret named
`HOMEBREW_TAP_TOKEN` in `wiccy46/aes67-tools`. Use a fine-grained GitHub token
with `Contents: Read and write` access to `wiccy46/homebrew-aes67`.

Manual local formula test flow:

1. Build release archives for each supported target. The release workflow builds
   these on native GitHub-hosted macOS and Ubuntu runners:

   ```bash
   AES67_RELEASE_TARGET=aarch64-apple-darwin bash scripts/package_release.sh
   AES67_RELEASE_TARGET=x86_64-unknown-linux-gnu bash scripts/package_release.sh
   ```

2. Upload each `target/release-packages/*.tar.gz` archive and `.sha256`
   checksum to the GitHub release tagged `v<VERSION>`. The release workflow
   does this automatically for normal releases.

3. In `packaging/homebrew/aes67-tools.rb`, replace `REPLACE_WITH_VERSION`
   with the value from `VERSION`, then replace each `REPLACE_WITH_*_SHA256`
   value with the first field from the matching `.sha256` file. The release workflow updates the live tap automatically.

4. Test locally:

   ```bash
   brew install --build-from-source ./packaging/homebrew/aes67-tools.rb
   brew test ./packaging/homebrew/aes67-tools.rb
   ```

The formula installs the single `aes67` command into Homebrew's `bin`
directory and examples under `pkgshare`.
