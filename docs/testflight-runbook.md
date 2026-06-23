# Finite Chat TestFlight Runbook

Finite Chat ships as its own App Store Connect app record.

## Product Identity

- App name: `Finite Chat`
- Bundle ID: `computer.finite.finitechat`
- SKU: `computer.finite.finitechat` or `finitechat-ios`
- Primary language: English
- Xcode project: `ios/FiniteChat.xcodeproj`
- Scheme: `FiniteChat`
- Signing team in `ios/project.yml`: `Y392XZ3MST`

This keeps the App Store Connect record aligned with the repo, bundle ID,
keychain/runtime naming, and product metadata.

## App Store Connect Setup

1. In App Store Connect, create a new iOS app record.
2. Select or create the Bundle ID `computer.finite.finitechat`.
3. Use `Finite Chat` as the app name.
4. Add beta test information before external testing:
   - beta app description
   - feedback email
   - contact information
   - "What to Test" notes
5. Complete required compliance metadata:
   - encryption/export compliance: answer that the app uses encryption, then
     follow Apple's questionnaire for standard/open encryption where no
     documentation is required
   - privacy nutrition labels
   - age rating
   - content rights
6. Start with an internal TestFlight group, then add an external group after the
   first uploaded build is viable.

External TestFlight should be treated as a review gate. The first build added to
an external testing group is sent to Beta App Review; later builds for the same
version may not require a full review.

## Xcode Cloud

Use Xcode Cloud for repeatable TestFlight delivery after the App Store Connect
record exists.

Suggested first workflow:

- Repository branch: `main` or the release branch used for Finite Chat builds
- Project: `ios/FiniteChat.xcodeproj`
- Scheme: `FiniteChat`
- Actions:
  - Test on current iOS Simulator
  - Archive for iOS
  - Distribute to internal TestFlight testers

The repo does not track generated iOS build artifacts. Xcode Cloud must run
`ios/ci_scripts/ci_post_clone.sh`, which:

1. Installs Rust, `protoc`, and XcodeGen if missing.
2. Adds the iOS Rust targets.
3. Runs `cargo run -q -p finitechat-rmp -- bindings swift --clean`.
4. Regenerates `ios/FiniteChat.xcodeproj` from `ios/project.yml`.

Apple requires Xcode Cloud custom scripts to live in a `ci_scripts` directory at
the same level as the Xcode project or workspace. Because the project path is
`ios/FiniteChat.xcodeproj`, the script lives at
`ios/ci_scripts/ci_post_clone.sh`.

## Preflight Checks

Before the first uploaded build:

```sh
cargo test --workspace
cargo run -q -p finitechat-rmp -- bindings swift --clean
(cd ios && xcodegen generate)
xcodebuild -project ios/FiniteChat.xcodeproj -scheme FiniteChat -configuration Release -sdk iphonesimulator build
```

Also verify:

- `CFBundleShortVersionString` and `CFBundleVersion` are bumped in
  `ios/Info.plist`.
- App icon assets are present in
  `ios/Sources/Assets.xcassets/AppIcon.appiconset`.
- Privacy permission copy in `ios/Info.plist` matches actual behavior.
- Production server configuration is intentional; the TestFlight build should
  not depend on a local development server unless the beta scope explicitly says
  so.
- Review notes explain that Finite Chat uses end-to-end encryption, Nostr
  identity material, camera QR scanning, microphone recording, speech
  transcription, and optional photo-library saves initiated by the user.
