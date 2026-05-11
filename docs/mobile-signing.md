# Twe mobile signing + store submission

Phase 39 sessions 4 + 5 + 8. The codebase ships the iOS / Android
build layouts (Phase 39 session 1); this document is the operator-
action recipe that turns those layouts into store-uploadable
artifacts. None of the steps below run on the Twe maintainer's
machine — every game shipper has their own developer accounts +
provisioning + keystore.

This doc deliberately keeps the mechanical steps short and links to
the canonical platform docs for the parts that change every year.

---

## iOS path

Producing a signed `.ipa` from `twec build --target ios`:

### Prerequisites

- A macOS host with Xcode 15+ installed.
- An Apple Developer account ($99/year).
- An iOS provisioning profile for your bundle identifier (the
  build layout's `Info.plist` ships with `dev.twe.<slug>` — change
  this to your real reverse-DNS in `twe.toml` before building).

### Steps

1. **Run `twec build --target ios examples/your_game/`.** Produces
   `dist/your_game-ios/Payload/your_game.app/` with `Info.plist` +
   `your_game.twebundle`. Plus `README.txt` documenting the chain
   below.
2. **Cross-compile the twec runtime for iOS:**
   ```sh
   rustup target add aarch64-apple-ios
   cd /path/to/twec-source
   cargo build --target aarch64-apple-ios --release
   ```
   Result: `target/aarch64-apple-ios/release/twec`. Copy this into
   `dist/your_game-ios/Payload/your_game.app/your_game` (rename to
   match the bundle's executable name, matching the `Info.plist`
   `CFBundleExecutable` field).
3. **Sign the bundle:**
   ```sh
   codesign --sign "Apple Distribution: Your Name (TEAMID)" \
            --entitlements your_game.entitlements \
            dist/your_game-ios/Payload/your_game.app
   ```
4. **Zip into an `.ipa`:**
   ```sh
   cd dist/your_game-ios
   zip -r your_game.ipa Payload
   ```
5. **Upload to App Store Connect** via Transporter or
   `xcrun altool`. TestFlight build appears within minutes once
   processing finishes.

### Known gotchas

- **Metal capability flag.** The `Info.plist` template includes
  `<string>metal</string>` in `UIRequiredDeviceCapabilities`. iPads
  + iPhones older than 6s/6 don't ship Metal — the App Store will
  reject submissions targeting them.
- **Landscape-only orientation.** The template restricts to
  landscape via `UISupportedInterfaceOrientations`. Portrait games
  must override.
- **Launch storyboard.** `UILaunchStoryboardName` defaults to
  `LaunchScreen`; create a minimal `LaunchScreen.storyboard` in the
  bundle root or App Store Connect will reject the build.

---

## Android path

Producing a signed `.aab` (or `.apk` for sideloading) from
`twec build --target android`:

### Prerequisites

- Android Studio (or command-line Android SDK + NDK r25 + Gradle 8).
- A keystore for app signing (`keytool -genkey ...`). Back this up
  off-machine — losing it locks you out of further releases.
- A Google Play Console account ($25 one-time).

### Steps

1. **Run `twec build --target android examples/your_game/`.** Produces
   `dist/your_game-android/app/src/main/{AndroidManifest.xml, assets/your_game.twebundle}`.
2. **Cross-compile the twec runtime for Android:**
   ```sh
   rustup target add aarch64-linux-android
   # Set ANDROID_NDK_HOME to your NDK install.
   export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/aarch64-linux-android21-clang"
   cd /path/to/twec-source
   cargo build --target aarch64-linux-android --release
   ```
   Result: `target/aarch64-linux-android/release/libtwec.so`. Copy
   this into `dist/your_game-android/app/src/main/jniLibs/arm64-v8a/libtwec.so`.
3. **Wrap with a Gradle module.** Open `dist/your_game-android/` in
   Android Studio. Create or copy a `build.gradle.kts` at the project
   root + an `app/build.gradle.kts` referencing the NativeActivity from
   the manifest. (Sketch:)
   ```kotlin
   plugins { id("com.android.application") version "8.2" }
   android {
       namespace = "dev.twe.your_game"
       compileSdk = 34
       defaultConfig { applicationId = "dev.twe.your_game"; minSdk = 24; targetSdk = 34 }
   }
   ```
4. **Sign + bundle:**
   ```sh
   ./gradlew bundleRelease
   # Outputs app/build/outputs/bundle/release/app-release.aab
   ```
5. **Upload to Play Console** via the web UI or `bundletool` CLI.
   Internal-track release appears within ~hours.

### Known gotchas

- **NDK API level 21.** The `aarch64-linux-android21-clang` flag
  matters; older NDK toolchains target API 19 which `rapier3d` does
  not support.
- **Keystore alias + passwords.** `./gradlew bundleRelease` will
  prompt for these if they're not set in `~/.gradle/gradle.properties`
  via `RELEASE_STORE_PASSWORD` + `RELEASE_KEY_PASSWORD`.
- **Target SDK refresh deadline.** Google bumps the required
  `targetSdk` annually. Check `https://developer.android.com/google/play/requirements/target-sdk`
  before each release; failing to bump locks your listing from
  update visibility.

---

## App Store / Play Store submission docs

This list isn't Twe-specific, but it's where shippers most often get
stuck and the docs change yearly. The references below are
authoritative as of 2026-05; check the official platform docs at
submit time.

### iOS

- App Store Review Guidelines: `https://developer.apple.com/app-store/review/guidelines/`
- Metadata + screenshots requirements: `https://developer.apple.com/app-store/product-page/`
- TestFlight beta groups: `https://developer.apple.com/testflight/`

### Android

- Play Console launch checklist: `https://support.google.com/googleplay/android-developer/answer/9859152`
- Content rating questionnaire: must be answered before public release.
- Data safety form: lists what data the game collects + sends. Twe
  v1.x default is "no data collection" — fill the form accordingly.

---

## What this doc is *not*

- A signing automation tool. CI integration via `cargo-dist` mobile
  target descriptors is a Phase 39 follow-on item — for now, the
  signing happens on a developer's macOS host (iOS) or Linux+NDK
  host (Android).
- A monetization guide. IAP, ads, analytics are out of scope; the
  store submission packages this doc walks through ship a paid or
  free game with no in-app purchases.
- A platform-port engineering guide. Touch input, virtual joystick,
  safe-area inset handling — those are codebase deliverables of
  Phase 39 sessions 2-6, not packaging steps.
