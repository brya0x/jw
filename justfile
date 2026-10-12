# Recipes for jw. `just` lists them; `just ci` runs what CI runs.
# CI calls these same recipes, so a green `just ci` is a green PR.

set shell := ["bash", "-euo", "pipefail", "-c"]

# Gradle needs the SDK; a git-ignored android/local.properties also works.
export ANDROID_HOME := env("ANDROID_HOME", env("ANDROID_SDK_ROOT", home_directory() / "Library/Android/sdk"))

default:
    @just --list

# --- Rust -------------------------------------------------------------------

build:
    cargo build --workspace

test:
    cargo test --workspace --locked

clippy:
    cargo clippy --workspace --all-targets --locked -- -D warnings

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

# Put the new jw on PATH.
install:
    cargo install --path crates/jw --locked

# What the Rust job in CI runs.
rust: fmt-check clippy test

# --- Mobile: the PC side ----------------------------------------------------

# The bridge crate, the daemon's mobile service and the binary's mobile tests.
mobile-rust:
    cargo test --locked -p jw-web -p jw-daemon
    cargo test --locked -p jw --test mobile --test mobile_cli --test web

# --- Mobile: the Android app ------------------------------------------------

android-build:
    cd android && ./gradlew --no-daemon assembleDebug

android-test:
    cd android && ./gradlew --no-daemon :proto:test :term:test :app:testDebugUnitTest

android-lint:
    cd android && ./gradlew --no-daemon :app:lintDebug

# What the Android job in CI runs.
android: android-build android-test android-lint

# Everything the phone path needs: the PC side and the app.
mobile: mobile-rust android

# --- Both -------------------------------------------------------------------

# What CI runs: the Rust job, then the Android job.
ci: rust android
