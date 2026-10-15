set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

default:
    @just --list

# 检查命令统一由 ci/check.sh 定义；本页保留稳定、可发现的用户入口。
fmt:
    @bash ci/check.sh fmt

fmt-fix:
    cargo fmt --all

# 文档链接、锚点、导航覆盖和包 README 的静态门禁，不运行产品或行为测试。
docs-check:
    @bash ci/check.sh docs

repository-policy-check:
    @bash ci/check.sh repository

layout-check:
    @bash ci/check.sh layout

config-example-check:
    @bash ci/check.sh config

i18n-check:
    @bash ci/check.sh i18n

# 联网审计锁定依赖公告；需 cargo-audit 0.22.2，独立于默认本地 verify。
dependency-audit:
    @bash ci/check.sh dependency-audit

# 静态扫描未使用的直接依赖；需 cargo-machete 0.9.2，不运行产品或测试。
dependency-unused:
    @bash ci/check.sh dependency-unused

check:
    @bash ci/check.sh check

feature-check:
    @bash ci/check.sh feature

lint:
    @bash ci/check.sh lint

wasm-check:
    @bash ci/check.sh wasm

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
package *args:
    @bash ci/release.sh package {{args}}

# 完整执行两类构建、制品门禁和发布装配；不会运行测试。
release-check *args:
    @bash ci/release.sh release-check {{args}}

run-public:
    cargo run -p auction-public-api --features postgres,web-embed

run-admin:
    cargo run -p auction-admin-api --features postgres,web-embed

run-worker:
    cargo run -p auction-worker --features postgres

check-config role="public":
    cargo run -p auctionctl -- check-config {{role}}

db-migrate:
    cargo run -p auctionctl --features postgres -- migrate

# 需要配套 dx CLI；WASM profile、payload 和工具边界见 ci/README.md。
web-build: web-build-public web-build-admin

web-build-public:
    @bash ci/web-bundle.sh public

web-build-admin:
    @bash ci/web-bundle.sh admin

web-serve-public:
    dx serve -p auction-web-public --platform web

web-serve-admin:
    dx serve -p auction-web-admin --platform web
