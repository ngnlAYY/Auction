set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    @just --list

fmt:
    cargo fmt --all -- --check

fmt-fix:
    cargo fmt --all

check:
    cargo check --locked --workspace --all-targets

feature-check:
    cargo check --locked -p auction-storage --all-targets --no-default-features --features postgres
    cargo check --locked -p auction-storage --all-targets --no-default-features --features sqlite
    cargo check --locked -p auction-cache --all-targets --no-default-features --features redis

lint:
    cargo clippy --locked --workspace --all-targets --all-features -- -D warnings

wasm-check:
    cargo check --locked -p auction-web --target wasm32-unknown-unknown --no-default-features --features hydrate

verify: fmt check feature-check lint wasm-check

run-public:
    cargo run -p auction-public-api

run-admin:
    cargo run -p auction-admin-api

run-worker:
    cargo run -p auction-worker

check-config role="public":
    cargo run -p auctionctl -- check-config {{role}}

db-migrate:
    cargo run -p auctionctl -- migrate

web-watch:
    cargo leptos watch -p auction-web
