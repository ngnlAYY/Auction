set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

# 列出全部可用入口，不含隐式构建或检查。
default:
    @just --list

# 唯一默认验证入口：策略（含源码棘轮与文档链接）→ 格式 → 编译/独立 feature/Clippy/WASM，不运行测试。
verify:
    @bash ci/checks/check.sh all fmt check feature dependency-versions lint wasm

# 显式运行默认 feature 下的纯逻辑、单元与本地替身测试；数据库测试必须独立隔离。
test:
    @cargo test --locked --workspace --all-targets

# 显式运行 PostgreSQL 18 测试；设置 AUCTION_TEST_DATABASE_URL 时复用该服务器，否则用本机 PG18 二进制启动并清理临时集群。
test-db:
    @cargo test --locked -p auction-storage --features test-db --tests -- postgres_

# 只检查 rustfmt 格式，不写文件。
fmt:
    @bash ci/checks/check.sh fmt

# 编译工作区全部已有目标；只编译不运行测试。
check:
    @bash ci/checks/check.sh check

# 分别验证 PostgreSQL、Redis，以及首次设置的空默认图和完整 feature 图。
feature-check:
    @bash ci/checks/check.sh feature

# 以 all-features 跑 Clippy，警告即失败。
lint:
    @bash ci/checks/check.sh lint

# 以 wasm32 Clippy 检查 Public/Admin 浏览器目标及共享层，警告即失败；不产出发布 payload。
wasm-check:
    @bash ci/checks/check.sh wasm

# 显式本地 Blaze 试验，参数逐项转发；不进入 verify 或发布依赖。
[positional-arguments]
blaze +args:
    @bash ci/tools/blaze.sh "$@"

# 删除全部编译产物与本地缓存；target/ 由 cargo 与发布脚本共同拥有。
clean:
    cargo clean

# 构建双端页面并生成固定 payload；输入未变时复用已有产物。
web-build:
    # Public 页面与其 payload 清单。
    @bash ci/release/web-bundle.sh public
    # Admin 页面与其 payload 清单。
    @bash ci/release/web-bundle.sh admin

# 不包含浏览器资源的 API-only 对照制品。
build-api:
    @bash ci/release/release.sh build-api

# 双端 web-embed 完整制品；源码身份未变时复用已有制品。
build-release:
    @bash ci/release/release.sh build-embedded

# 可选合并服务；先保留分体制品与资源隔离证明。
build-single: build-api build-release
    # 构建 auction-server 与独立 auction-cli。
    @bash ci/release/release.sh build-single
    # 核对双 surface payload 与独立 auction-cli。
    @bash ci/release/release.sh verify-single

# 核对 API-only/嵌入式制品、依赖边界与资源隔离。
verify-artifacts:
    @bash ci/release/release.sh verify-artifacts

# 完整构建、制品门禁与发布装配；不加参数不运行测试。
release-check *args:
    @bash ci/release/release.sh release-check {{args}}

# 只装配已构建制品；正式发布要求干净工作树。
package *args:
    @bash ci/release/release.sh package {{args}}

# 本地启动 Public，内嵌页面走 PostgreSQL。
run-public:
    cargo run --locked -p auction-public-api --features postgres,web-embed

# 本地启动 Admin，内嵌页面走 PostgreSQL。
run-admin:
    cargo run --locked -p auction-admin-api --features postgres,web-embed

# 本地启动 Worker，不内嵌页面。
run-worker:
    cargo run --locked -p auction-worker --features postgres

# 本地开发库执行内嵌迁移；普通启动不隐式迁移，本地不经 systemd 停机门禁。
[positional-arguments]
db-migrate config_dir="ops/config":
    cargo run --locked -p auction-cli --features postgres -- --config-dir "$1" db migrate --no-systemd

# 输出指定角色的有效配置安全投影，默认 public。
[positional-arguments]
config-show role="public" config_dir="ops/config":
    cargo run --locked -p auction-cli -- --config-dir "$2" config show "$1"

# 联合检查三角色配置；生产门禁另加 --strict。
[positional-arguments]
config-check config_dir="ops/config":
    cargo run --locked -p auction-cli --features postgres -- --config-dir "$1" config check
