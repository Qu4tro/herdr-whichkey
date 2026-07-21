# The push/PR gate, defined once. .github/workflows/ci.yml installs just and
# runs `just ci` — it holds no copy of these commands, so local and CI cannot
# drift. Change a check here and CI changes with it.
#
# Split into named recipes so a failure says which check failed and you can
# rerun that one alone.
#
# Keep to syntax just 1.21 understands — that is what noble universe ships and
# what CI installs. Newer syntax works locally and fails there.

# Bare `just` lists rather than runs: typing it to see what is here should not
# kick off a full compile-and-test. Ask for the gate by name.

# list the recipes
default:
    @just --list

# everything the gate runs, in order
ci: fmt lint test shellcheck

# rustfmt, per rustfmt.toml
fmt:
    cargo fmt --check

# Warnings are denied, not warned about — a warning nobody sees is not a gate.
# --locked here and in test: a green run must describe the versions pinned in
# Cargo.lock, not whatever resolved that morning.

# clippy over every target and feature, warnings fatal
lint:
    cargo clippy --all-targets --all-features --locked -- -D warnings

# the unit tests
test:
    cargo test --locked

# build.sh is what herdr runs on plugin install/update, so a bug there breaks
# installs rather than the menu.

# lint the shell scripts
shellcheck:
    shellcheck scripts/*.sh

# clippy first, then fmt over whatever it rewrote. --allow-dirty/--allow-staged
# because the point is to run this on work in progress; commit or stash first
# if you want an undo.

# apply what fmt and lint can fix on their own
fix:
    cargo clippy --fix --all-targets --all-features --locked --allow-dirty --allow-staged
    cargo fmt
