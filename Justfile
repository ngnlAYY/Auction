set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    @just --list

fmt:
    cargo fmt --all -- --check

fmt-fix:
    cargo fmt --all

check:
    cargo check --workspace --all-targets

lint:
    cargo clippy --workspace --all-targets --all-features -- -D warnings

verify: fmt check lint

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
