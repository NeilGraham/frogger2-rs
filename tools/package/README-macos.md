# macOS packaging

`tools/package/package.sh macos` (run on a Mac with the Xcode command line tools) builds
`frogger2`, `f2-import`, `f2-viewer`, wraps them in `Frogger2rs.app` and writes
`target/package/frogger2-rs-<ver>-macos-<arch>.zip` (with `ditto`, the release
asset) and `.tar.gz`. The release builds `F2_MACOS_UNIVERSAL=1` (`<arch>` = `universal`).

| Variable | Effect |
|---|---|
| `F2_MACOS_UNIVERSAL=1` | build `aarch64-apple-darwin` + `x86_64-apple-darwin` and `lipo` them (needs both targets: `rustup target add aarch64-apple-darwin x86_64-apple-darwin`) |
| `F2_MACOS_TARGET=x86_64-apple-darwin` | one architecture other than the host's (the release workflow builds `arm64` and `x86_64` this way on one Apple silicon runner) |
| `F2_PKG_VERSION=0.1.0` | archive names with this version instead of `<version>-<commit>` |
| `F2_SKIP_BUILD=1` | package already-built binaries (`$CARGO_TARGET_DIR` honoured) |
| `F2_CODESIGN_ID` | signing identity; default `-` (ad-hoc) |

Bundle layout: `Contents/MacOS/{frogger2,f2-import,f2-viewer}` (the app launches `frogger2`),
`Contents/Info.plist` (`NSHighResolutionCapable`, minimum macOS 11), `Contents/Resources/`
(icon, licenses). The icon comes from the pre-rendered `packaging/icon/png/*.png`; they are converted to
`.icns` with `iconutil` (part of Xcode command line tools).

## Where things live

- The zip is portable: `portable.txt` sits beside the app, so the imported game (`data/`), `config.toml`,
  `saves/` and `mods/` live in that same folder. Delete `portable.txt` to use
  `~/Library/Application Support/frogger2-rs/` instead (or set `$F2_DATA_ROOT`).
- Import from Terminal: `Frogger2rs.app/Contents/MacOS/f2-import /path/to/disc.cue`.

## Signing and Gatekeeper

- Apple Silicon refuses to run unsigned arm64 code, so the script always signs; the default is an
  **ad-hoc** signature (`codesign -s -`). Ad-hoc apps run on the machine that built them; a
  downloaded copy is quarantined: right-click > Open once, or
  `xattr -dr com.apple.quarantine Frogger2rs.app`.
- With a Developer ID (`F2_CODESIGN_ID="Developer ID Application: Name (TEAMID)"`) the script
  signs with the hardened runtime (`--options runtime --timestamp`), which notarisation requires.
  No entitlements are needed: the game uses no JIT, and Metal, CoreAudio, game controllers and
  sockets (netplay) work under the hardened runtime.

## Notarisation (not automated; needs a paid Apple Developer account)

```sh
ditto -c -k --keepParent target/package/stage/macos/<name>/Frogger2rs.app Frogger2rs.zip
xcrun notarytool submit Frogger2rs.zip --apple-id you@example.com --team-id TEAMID \
  --password <app-specific password> --wait        # or --keychain-profile <profile>
xcrun stapler staple target/package/stage/macos/<name>/Frogger2rs.app
spctl --assess --type execute -vv target/package/stage/macos/<name>/Frogger2rs.app
```

Then re-create the zip (or a `.dmg` with `hdiutil create -srcfolder ... -format UDZO`).
In CI, the signing certificate (.p12) and notarytool credentials would be repository secrets
imported into a temporary keychain; the `macos` job of `.github/workflows/release.yml`
currently only ad-hoc signs.

## Testing status

The macOS code paths are compile-checked from Linux
(`cargo check --workspace --all-targets --no-default-features --target aarch64-apple-darwin`,
also in CI) and built and tested natively by the `macos-14` CI job. Linking, the `.app` bundle
and runtime behaviour (Metal, Retina sizing, Cmd shortcuts, controllers) have not been tried on a
real Mac yet. See `docs/PLATFORMS.md`.
