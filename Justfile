set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    @just --list

fmt:
    cargo fmt --all -- --check

fmt-fix:
    cargo fmt --all

repository-policy-check:
    @bash ci/check-policy.sh repository

layout-check:
    @bash ci/check-policy.sh layout

config-example-check:
    @bash ci/check-policy.sh config

i18n-check:
    @bash ci/check-policy.sh i18n

check:
    cargo check --locked --workspace --all-targets

feature-check:
    cargo check --locked -p auction-storage --all-targets --no-default-features --features postgres
    cargo check --locked -p auction-storage --all-targets --no-default-features --features sqlite
    cargo check --locked -p auction-cache --all-targets --no-default-features --features redis

lint:
    cargo clippy --locked --workspace --all-targets --all-features -- -D warnings

wasm-check:
    cargo check --locked -p auction-web-public --target wasm32-unknown-unknown
    cargo check --locked -p auction-web-admin --target wasm32-unknown-unknown

verify: fmt repository-policy-check layout-check config-example-check i18n-check check feature-check lint wasm-check

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

# 前端产物构建需要 dx CLI（cargo install dioxus-cli --locked）。
# dx 会按需从 npm 注册表下载 esbuild 作为构建期工具；它不是运行时依赖，也不引入
# Node 运行时或第二套前端构建链。这里使用 dx bundle 先构建再复制到稳定的嵌入目录，
# 使 debug/release 后端都能读取同一份产物。auction-http 使用 allow_missing 内嵌产物，
# 因此未执行这些命令时后端仍可编译，只是运行时静态资源返回 503。
web-build: web-build-public web-build-admin

web-build-public:
    rm -rf target/dx/build/auction-web-public/web
    dx bundle -p auction-web-public --platform web --out-dir target/dx/build/auction-web-public/web --locked

web-build-admin:
    rm -rf target/dx/build/auction-web-admin/web
    dx bundle -p auction-web-admin --platform web --out-dir target/dx/build/auction-web-admin/web --locked

web-serve-public:
    dx serve -p auction-web-public --platform web

web-serve-admin:
    dx serve -p auction-web-admin --platform web
