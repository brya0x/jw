# `just ci`: the same recipes CI runs. Green before every commit.
default: ci

ci: fmt-check clippy test android-check

fmt-check:
    cargo fmt --all --check

clippy:
    cargo clippy --workspace --all-targets --locked -- -D warnings

test:
    cargo test --workspace --locked

# The Kotlin side of the contract: the same fixtures, parsed and re-written by Kotlin.
android-check:
    cd android && ./gradlew --no-daemon :contract:test :client-core:test :app:lintDebug :app:assembleDebug

# Build jw and put it in ~/.cargo/bin. The running daemon keeps the old binary until `jw stop`.
install:
    cargo install --path crates/jw --locked

# Build the debug app and install it on the phone that adb sees (it keeps the app's data).
android:
    cd android && ./gradlew --no-daemon :app:installDebug
