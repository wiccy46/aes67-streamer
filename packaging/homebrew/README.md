# Homebrew Formula

This directory contains the Homebrew formula template for `aes67-tools`.

The normal release path is the manual GitHub Actions workflow
`.github/workflows/release-run.yml`. It updates the live tap repository
`wiccy46/homebrew-aes67` after the GitHub release asset has been uploaded.

Release flow:

1. Build release archives for each supported target:

   ```bash
   AES67_RELEASE_TARGET=aarch64-apple-darwin bash scripts/package_release.sh
   AES67_RELEASE_TARGET=x86_64-apple-darwin bash scripts/package_release.sh
   AES67_RELEASE_TARGET=x86_64-unknown-linux-gnu bash scripts/package_release.sh
   AES67_RELEASE_TARGET=aarch64-unknown-linux-gnu bash scripts/package_release.sh
   ```

2. Upload each `target/release-packages/*.tar.gz` archive and `.sha256`
   checksum to the GitHub release tagged `v<VERSION>`.

3. In `packaging/homebrew/aes67-tools.rb`, replace each
   `REPLACE_WITH_*_SHA256` value with the first field from the matching
   `.sha256` file.

4. Test locally:

   ```bash
   brew install --build-from-source ./packaging/homebrew/aes67-tools.rb
   brew test ./packaging/homebrew/aes67-tools.rb
   ```

The formula installs `aes67-streamer` and `aes67-player` into Homebrew's `bin`
directory and installs examples under `pkgshare`.
