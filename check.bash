set -ex

cargo +nightly fmt
cargo clippy
