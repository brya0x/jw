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

# Build the test install (N190) and put it next to jw as `jwtest`: another identity compiled in
# (identity.test.toml), its own target directory, and the binary `jw` is never written.
jwtest:
    #!/usr/bin/env bash
    set -euo pipefail
    IDENTITY_FILE="{{justfile_directory()}}/identity.test.toml" \
        cargo build --release --locked -p jw --target-dir target/jwtest
    bin="${CARGO_INSTALL_ROOT:-${CARGO_HOME:-$HOME/.cargo}}/bin"
    mkdir -p "$bin"
    install -m 755 target/jwtest/release/jw "$bin/jwtest"
    said="$("$bin/jwtest" --version | head -n 1)"
    case "$said" in
        jwtest\ *) echo "$said ($bin/jwtest)" ;;
        *) echo "jwtest --version says «$said»: not the test identity" >&2; exit 1 ;;
    esac
