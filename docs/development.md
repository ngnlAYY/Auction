# 开发指南

[文档导航](README.md) · [架构](architecture.md) · [协作规范](../AGENTS.md) · [CI 与发布](../ci/README.md)

## 环境

| 项 | 要求 |
| --- | --- |
| 平台 | Linux x86_64（原生程序在其他平台编译期报错；浏览器包编译到 `wasm32-unknown-unknown`） |
| Rust | 工具链固定在 [`rust-toolchain.toml`](../rust-toolchain.toml)（1.99.0，含 clippy、rustfmt 与 wasm32 目标）；MSRV 1.94.1 由 CI 单独检查 |
| 命令 | `just`、`bash`、`ripgrep`（`rg`）、`perl`（策略门禁需要） |
| 数据库 | PostgreSQL 18（运行与运维命令需要；静态检查不需要） |
| 页面构建 | 受控的 `dx`（Dioxus CLI 0.7）、`wasm-bindgen`、`wasm-opt`，由 `bash ci/tools/bootstrap.sh install` 安装到 `target/build-tools`，见 [`ci/tools/README.md`](../ci/tools/README.md) |

直接调用负责依赖解析的 Cargo 命令时加 `--locked`；本地 Justfile 入口、CI 检查与发布构建已固定该参数。本地配置命令通过独立参数传递目录，包含空格的路径须在调用 `just` 时加引号。如需本机编译加速（sccache、mold 等），只在用户级 `~/.cargo/config.toml` 中配置，不改仓库 profile。

## 常用命令

本文所有命令都在仓库根目录执行，命令中的路径相对于仓库根目录。

```bash
just                 # 列出全部入口
just verify          # 默认验证：策略门禁 → fmt → check → feature → clippy → wasm（不运行测试）
just test            # 默认 feature 下的纯逻辑、单元与本地替身测试
just test-db         # 显式 PG18 集成测试，要求 AUCTION_TEST_DATABASE_URL
just fmt             # 只检查格式
just lint            # clippy -D warnings（all-features）
just wasm-check      # wasm32 目标 Clippy 检查两个浏览器包（-D warnings）
just web-build       # 构建 Public/Admin 页面 payload（需要受控 dx）
just web-size        # 只核对两端 Web payload 体积预算（不构建）
just run-public      # cargo run --locked -p auction-public-api --features postgres,web-embed
just run-admin
just run-worker
```

`just verify` 等价于 `bash ci/checks/check.sh all fmt check feature dependency-versions lint wasm`，各项含义见 [`ci/README.md`](../ci/README.md)。提交前至少通过它。

## 本地运行

### 1. 准备数据库

```bash
createuser --pwprompt auction
createdb --owner auction auction
```

或者用维护凭据通过 `auction-cli db create` 建库（见 [auction-cli](../apps/auction-cli/README.md#建库)）。

### 2. 准备配置

建议把开发配置放在**仓库之外**的私有目录（例如 `~/.config/auction-dev/`），避免误提交敏感文件：

```bash
dir=~/.config/auction-dev
mkdir -p -m 700 "$dir/runtime"
cp ops/config/examples/{public,admin,worker,common,secrets}.toml "$dir/"
cp ops/config/examples/runtime/admin.toml "$dir/runtime/"
chmod 600 "$dir/secrets.toml"; chmod 640 "$dir/runtime/admin.toml"
```

然后编辑 `secrets.toml` 的 `[database]`、`admin_actor`/`admin_secret`，按需生成数据密钥（32 字节随机数的 Base64）：

```bash
openssl rand -base64 32
```

也可以选择首次设置向导生成整套配置（需要 `postgres,web-embed` 构建），替代上面的手工创建步骤：先令 `dir` 指向一个尚不存在的目录，并预先建立符合[首次设置权限要求](../ops/config/README.md#示例与首次设置)的直接父目录，再运行 `cargo run --locked -p auction-admin-api --features postgres,web-embed -- setup --output-dir "$dir"`。

检查配置：

```bash
just config-check "$dir"            # 三角色联合检查
just config-show worker "$dir"      # 单角色安全投影
```

### 3. 迁移

```bash
just db-migrate "$dir" "$owner_secrets"   # auction-cli db migrate --owner-secrets … --no-systemd
just db-grants  "$dir" "$owner_secrets"   # 重放运行账号最小权限（迁移 000081）
```

普通启动从不隐式迁移；schema 不是当前版本时进程不会就绪。服务账号是迁移 081 授权的
`auction_runtime`（无 DDL、无 `TRUNCATE`、只追加表仅 `INSERT`），迁移与恢复必须用 owner-only
的迁移凭据（`--owner-secrets`），本地模板见 `ops/config/examples/secrets/postgres-owner.toml`。

### 4. 启动

```bash
just web-build                                        # 首次或前端改动后
AUCTION_CONFIG_FILE=$dir/worker.toml just run-worker
AUCTION_CONFIG_FILE=$dir/admin.toml  just run-admin
AUCTION_CONFIG_FILE=$dir/public.toml just run-public
```

默认 Public 监听 `127.0.0.1:8090`、Admin 监听 `127.0.0.1:8091`。未构建页面时页面路由返回 503，API 仍可用。Worker 默认不启用任何外部集成（MoogleMail、论坛、告警），本地开发无需外部凭据；联调 MoogleMail 时可在 `[worker.mooglemail]` 设置 `keyenv_file` 指向本地 `.keyenv`（只接受 `hv-ck-key1`、`hv-ck-key2` 两个键，Worker 使用 `hv-ck-key1`）。

也可以用 `auction-server --config-dir <目录>` 一次启动三个角色（需要启用 Worker control 并允许当前 UID）。

## 代码约定

详见 [`AGENTS.md`](../AGENTS.md)，要点：

- **目录布局**（`ci/checks/check.sh layout` 强制）：`src/` 根目录只能有 `lib.rs`/`main.rs`，实现放在按职责命名的子目录；同一目录不能有 3 个及以上同前缀的源码文件；文件与目录名使用 snake_case。
- **依赖方向**：domain ← application ← 适配器 ← apps；前端只依赖 contracts 与 web-shared。
- **命令**：写操作带稳定 `command_id`、请求摘要与期望版本，在一个事务内写入事实、结果、审计与事件。
- **注释与文档**：中文；模块用 `//!`，公共项用 `///`，复杂分支说明原因、不变量与取舍。
- **配置示例**：[`ops/config/examples/`](../ops/config/examples) 中每个赋值字段前必须紧邻含“可选”或“必填”的中文注释（`ci/checks/check.sh config`）。示例同时是首次设置生成器的模板，修改后须保证生成的配置仍能通过 loader 校验。
- **前端文案**：全部走 `i18n/*.ftl`，五种语言键集合一致，代码引用的键必须存在于 `en.ftl`，键名不得含点号（`ci/checks/check.sh i18n`）。
- **装备诊断闭集**：装备解析诊断原因在 Rust、SQL 约束、Admin 文案之间必须一致（`ci/checks/check.sh equipment-contract`）。
- **源码棘轮与文档链接**：`just verify` 包含 `ratchet` 与 `markdown-links`；模式计数只能下降，基线须随清理改动更新；相对链接及同页、跨页锚点必须有效，口径见[静态门禁说明](../ci/checks/README.md)。

## 测试

`just verify` 继续只做格式、编译与静态分析；测试使用独立入口，不并入默认门禁。行为变更必须随功能维护并运行受影响范围的测试，至少覆盖正反例与所修复问题的回归断言。已批准的 W6 类别包括纯逻辑、属性/快照、脱敏页面回放、PostgreSQL 18 集成，以及本地 MoogleMail/论坛替身；其他行为验证仍按 [AGENTS.md](../AGENTS.md#验证范围) 的授权范围执行。

```bash
just test
# 等价于：cargo test --locked --workspace --all-targets

just test-db
# 未设置 AUCTION_TEST_DATABASE_URL 时用本机 PG18 二进制启动临时集群，再执行：
# cargo test --locked -p auction-storage --features test-db --tests -- postgres_
```

`just test` 使用默认 feature，运行单元测试及无需数据库的集成测试（包括本地替身）；不运行文档测试，不要求 PostgreSQL，不连接真实游戏或论坛账号。可按改动所属 owner 缩小范围，例如 `cargo test --locked -p auction-domain --lib`；交付说明写出实际命令及未覆盖范围。

`just test-db` 启用 `auction-storage` 的 `test-db` feature。设置 `AUCTION_TEST_DATABASE_URL` 时复用该 URL 所在服务器，只创建与删除夹具自己的模板库和测试库，不改写 URL 指向的库；未设置时依次用 `AUCTION_TEST_PG_BIN`、`PATH`、`/usr/lib/postgresql/18/bin` 的 PostgreSQL 18 二进制在临时目录启动一次性集群。URL 只通过环境传入，不写入文档、日志或提交的配置；夹具校验服务器 major 为 18。涉及锁、隔离级别、幂等或迁移的断言必须在真实 PG18 上执行。

夹具先在新建空库上执行全部内嵌迁移得到模板库，再由每个测试 `CREATE DATABASE ... TEMPLATE` 克隆独立库；测试库析构时执行 `DROP DATABASE ... WITH (FORCE)`，临时集群在最后一个测试释放共享句柄后停止服务并删除数据目录。`postgres_` 前缀是数据库测试的命名选择契约。

当前骨架只有 `crates/auction-storage/tests/postgres_storage.rs` 的空库迁移、模板克隆与 `WorldReader` 存储端口冒烟测试；157 个存储端口与运行时 SQL 尚未逐方法覆盖，不能据此声称数据库覆盖完整。数据库测试只在 `just test-db` 运行，默认 `just test` 不要求数据库；后续按 owner 扩展时保持同一入口、临时集群与逐测试独立库。

## 数据库变更

1. 在 [`migrations/postgres/`](../migrations/postgres) 新增迁移文件 `NNNNNN_<名称>.up.sql`（编号连续），见[迁移说明](../migrations/postgres/README.md)。
2. 每次新增迁移都把 [`ops/deploy/retention-policy.v1.json`](../ops/deploy/retention-policy.v1.json) 的 `database_schema_version` 更新为新的最高版本；新增表还须加入对应数据族。
3. 同步 [数据库契约](contracts/database.md)。

## 接口变更

- 新增或修改路由：同步 `auction-http` 路由表、`auction-contracts` DTO、两端前端与[路由契约](contracts/routes.md)；新增页面还要修改前端 `Route` 枚举与 [`crates/auction-http/src/assets/routes.rs`](../crates/auction-http/src/assets/routes.rs)。
- 新增实时事件：更新应用层事件注册表、浏览器解码器与[实时事件契约](contracts/realtime.md)。

## 文档

文档使用中文，Markdown 链接目标按当前文档所在目录计算相对路径，必要时使用 `../`；正文及链接显示文字中的仓库路径仍以仓库根为起点（如 `docs/contracts/routes.md`、`crates/auction-http/src/assets/routes.rs`）。每项事实只在一个 owner 文档中详细维护，其他页面摘要并链接。修改代码时同步相关 README 与契约文档；默认静态门禁离线检查文档相对链接与锚点，移动或重命名时须同步引用。
