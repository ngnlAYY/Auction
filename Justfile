set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    @just --list

fmt:
    cargo fmt --all -- --check

fmt-fix:
    cargo fmt --all

# 文档链接、导航覆盖和决策笔记的静态门禁，不运行产品或行为测试。
docs-check:
    @bash ci/check-docs.sh

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

verify: fmt docs-check repository-policy-check layout-check config-example-check i18n-check check feature-check lint wasm-check

# 构建包含前端资源的 PostgreSQL 完整档可部署制品。
build: build-release

# 构建不包含浏览器资源的 API-only 制品。
build-api:
    @bash ci/release.sh build-api

# 构建只包含各自 surface 浏览器资源的发布制品；脚本会先重建两端优化 payload。
build-release:
    @bash ci/release.sh build-embedded

# 构建可选合并服务，先保留分体制品与资源隔离证明。
build-single: build-api build-release
    @bash ci/release.sh build-single
    @bash ci/release.sh verify-single

# 核对 API-only、嵌入资源、依赖图和 Public/Admin 制品隔离。
verify-artifacts:
    @bash ci/release.sh verify-artifacts

# 只装配并核验已构建的发布制品；正式发布要求干净工作树。
package *args: verify-artifacts
    @bash ci/release.sh package {{args}}

# 完整执行两类构建、制品门禁和发布装配；不会运行测试。
release-check *args:
    @bash ci/release.sh release-check {{args}}

run-public:
    cargo run -p auction-public-api --features web-embed

run-admin:
    cargo run -p auction-admin-api --features web-embed

run-worker:
    cargo run -p auction-worker

check-config role="public":
    cargo run -p auctionctl -- check-config {{role}}

db-migrate:
    cargo run -p auctionctl -- migrate

# 前端产物构建需要 dx CLI（cargo install dioxus-cli --locked）。
# dx 会按需从 npm 注册表下载 esbuild 作为构建期工具；它不是运行时依赖，也不引入
# Node 运行时或第二套前端构建链。发布构建使用专用 WASM 优化 profile，清理该 surface
# 的 Dioxus 中间输出后复制到稳定嵌入目录，并生成绑定 payload 的制品标记。
web-build: web-build-public web-build-admin

web-build-public:
    @bash ci/web-bundle.sh public

web-build-admin:
    @bash ci/web-bundle.sh admin

web-serve-public:
    dx serve -p auction-web-public --platform web

web-serve-admin:
    dx serve -p auction-web-admin --platform web
