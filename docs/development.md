# 开发指南

[docs/README.md](README.md) · [docs/architecture.md](architecture.md) · [AGENTS.md](../AGENTS.md) · [ci/README.md](../ci/README.md)

## 环境

| 项 | 要求 |
| --- | --- |
| 平台 | Linux x86_64（原生程序在其他平台编译期报错；浏览器包编译到 `wasm32-unknown-unknown`） |
| Rust | 工具链固定在 [rust-toolchain.toml](../rust-toolchain.toml)（1.99.0，含 clippy、rustfmt 与 wasm32 目标）；MSRV 1.94.1 由 CI 单独检查 |
| 命令 | `just`、`bash`、`ripgrep`（`rg`）、`perl`（策略门禁需要） |
| 数据库 | PostgreSQL 18（运行与运维命令需要；静态检查不需要） |
| 页面构建 | 受控的 `dx`（Dioxus CLI 0.7）、`wasm-bindgen`、`wasm-opt`，由 `bash ci/tools/bootstrap.sh install` 安装到 `target/build-tools`，见 [ci/tools/README.md](../ci/tools/README.md) |

直接调用负责依赖解析的 Cargo 命令时加 `--locked`；本地 Justfile 入口、CI 检查与发布构建已固定该参数。本地配置命令通过独立参数传递目录，包含空格的路径须在调用 `just` 时加引号。如需本机编译加速（sccache、mold 等），只在用户级 `~/.cargo/config.toml` 中配置，不改仓库 profile。

## 常用命令

本文所有命令都在仓库根目录执行，命令中的路径相对于仓库根目录。

```bash
just                 # 列出全部入口
just verify          # 默认验证：策略门禁 → fmt → check → clippy → wasm（不运行测试）
just test            # 默认 feature 下的纯逻辑、单元与本地替身测试
just test-db         # 显式 PG18 集成测试，要求 AUCTION_TEST_DATABASE_URL
just test-db-compat  # 只验证迁移 compatible 声明（真实 PG18，沿迁移链）
just fmt             # 只检查格式
just lint            # clippy -D warnings（all-features）
just wasm-check      # wasm32 目标 Clippy 检查两个浏览器包（-D warnings）
just web-build       # 构建 Public/Admin 页面 payload（需要受控 dx）
just web-size        # 只核对两端 Web payload 体积预算（不构建）
just run-public      # cargo run --locked -p auction-public-api --features web-embed
just run-admin
just run-worker
```

`just verify` 等价于 `bash ci/checks/check.sh all fmt check dependency-versions lint wasm`，各项含义见 [ci/README.md](../ci/README.md)。它是纯静态入口：只读源码、配置示例与文档，不运行 `just test`/`just test-db`、不连接数据库、不启动服务、不访问网络（`ratchet`、`markdown-links` 等检查均离线）。因此编译与门禁通过不等于行为已验证，提交前至少通过它。Justfile 中的 `db-migrate`、`db-grants`、`config-check` 等本地命令会实际连接数据库或读取真实配置，不属于静态门禁。

## 本地运行

### 1. 准备数据库

```bash
createuser --pwprompt auction
createdb --owner auction auction
```

或者用维护凭据通过 `auction-cli db create` 建库（见 [apps/auction-cli/README.md#建库](../apps/auction-cli/README.md#建库)）。

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

也可以选择首次设置向导生成整套配置（需要 `web-embed` 构建），替代上面的手工创建步骤：先令 `dir` 指向一个尚不存在的目录，并预先建立符合[ops/config/README.md#示例与首次设置](../ops/config/README.md#示例与首次设置)的直接父目录，再运行 `cargo run --locked -p auction-admin-api --features web-embed -- setup --output-dir "$dir"`。

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

详见 [AGENTS.md](../AGENTS.md)，要点：

- **目录布局**（`ci/checks/check.sh layout` 强制）：`src/` 根目录只能有 `lib.rs`/`main.rs`，实现放在按职责命名的子目录；同一目录不能有 3 个及以上同前缀的源码文件；有子模块的模块以 `name/mod.rs` 为入口，不与同名 `name.rs` 并存；文件与目录名使用 snake_case。
- **依赖方向**：domain ← application ← 适配器 ← apps；前端只依赖 contracts 与 web-shared。
- **命令**：写操作带稳定 `command_id`、请求摘要与期望版本，在一个事务内写入事实、结果、审计与事件。
- **注释与文档**：中文；每个非测试源码文件以 `//!` 说明模块职责（`ci/checks/check.sh module-docs` 强制），公共项用 `///`，复杂分支说明原因、不变量与取舍。
- **配置示例**：[ops/config/examples](../ops/config/examples) 中每个赋值字段前必须紧邻含“可选”或“必填”的中文注释（`ci/checks/check.sh config`）。示例同时是首次设置生成器的模板，修改后须保证生成的配置仍能通过 loader 校验。
- **前端文案**：全部走 `i18n/*.ftl`，五种语言键集合一致，代码引用的键必须存在于 `en.ftl`，键名不得含点号（`ci/checks/check.sh i18n`）。
- **装备诊断闭集**：装备解析诊断原因在 Rust、SQL 约束、Admin 文案之间必须一致（`ci/checks/check.sh equipment-contract`）。
- **源码棘轮与文档链接**：`just verify` 包含 `ratchet` 与 `markdown-links`；模式计数只能下降，基线须随清理改动更新；相对链接及同页、跨页锚点必须有效，口径见[ci/checks/README.md](../ci/checks/README.md)。

## 测试

`just verify` 只做格式、编译与静态分析；测试使用独立入口，不并入默认门禁。行为变更必须随功能维护并运行受影响范围的测试，至少覆盖正反例与所修复问题的回归断言；已批准的测试类别与其他行为验证的授权范围见 [AGENTS.md#验证范围](../AGENTS.md#验证范围)。

```bash
just test        # cargo test --locked --workspace --all-targets（默认 feature）
just test-db     # 真实 PostgreSQL 18 集成测试，见下文
```

`just test` 运行单元测试及无需数据库的集成测试（包括本地替身）；不运行文档测试，不要求 PostgreSQL，不连接真实游戏或论坛账号，也不得隐式启动数据库集群。可按改动所属 owner 缩小范围，例如 `cargo test --locked -p auction-domain --lib`；交付说明写出实际命令及未覆盖范围。

`just test-db` 启用 `auction-storage` 的 `test-db` feature，断言涉及锁、隔离级别、幂等或迁移时必须在真实 PG18 上执行。`crates/auction-storage/src` 内另有少量以 `postgres_` 命名、标注 `#[ignore]` 的用例：它们只在事务内写连接私有临时表，设置了 `AUCTION_TEST_DATABASE_URL` 时由同一入口追加运行，未设置时明确跳过。设置 `AUCTION_TEST_DATABASE_URL` 时复用该 URL 所在服务器，只创建与删除夹具自己的模板库和测试库，不改写 URL 指向的库；该维护角色需要 `CREATEDB` 与 `CREATEROLE`（CI 显式授予，生产运行账号不授予）。未设置时依次用 `AUCTION_TEST_PG_BIN`、`PATH`、`/usr/lib/postgresql/18/bin` 的二进制在临时目录启动一次性集群。URL 只经环境传入，不写入文档、日志或提交的配置。该入口不得并入 `just verify` 或 `just test`：需要真实 PostgreSQL 语义的断言只能在这里运行，并保持独立数据库、独立角色与清理后的临时集群。

夹具先在空库上执行全部内嵌迁移得到模板库，再按测试 `CREATE DATABASE ... TEMPLATE` 克隆；析构时 `DROP DATABASE ... WITH (FORCE)`，临时集群在最后一个测试释放句柄后停止并删除数据目录。`postgres_` 前缀是数据库测试的命名选择契约。夹具另建只属于 `auction_runtime` 的临时登录角色，`TestDatabase::store()` 默认以它连接，用于证明最小权限下事务可以提交；`maintenance_store()` 只用于迁移、DDL 夹具与必须越过运行账号权限的断言读取，以维护账号驱动被测端口会掩盖运行账号缺权。

本地替身由 `crates/auction-test-support` 提供，只绑定 IPv4 回环、以脱敏语料回放，绝不连接真实账号；`crates/auction-integrations/tests/` 的邮件与论坛替身用真实客户端覆盖读取、账号忙、Cookie 失效、超时、协议漂移与写入结果分类，`mooglemail_maintenance_contention.rs` 复现“完整观察与 Sent 维护共用请求门导致维护超时”。`crates/auction-integrations/src/clients/mooglemail/sender/baseline/tests.rs` 把 Sent 页数上限设为 1，覆盖发送基线在 `full`/`shadow` 下的失败关闭、`watermark` 的边界取得与写后候选；`apps/auction-worker/src/bootstrap/shutdown/tests.rs` 在发送 POST 中途停止 Worker，验证结果落库后才退出、准备期工作立即放弃、预算耗尽只留 sending 围栏且不重发。替身未采集的远端页面、Worker 完整投递 runtime 与论坛写入成功页仍无替身驱动的用例；存储端口方法覆盖率用 `bash ci/checks/check.sh port-coverage` 统计，该报告不并入 `just verify`（口径见[ci/checks/README.md#存储端口覆盖率报告不并入-just-verify](../ci/checks/README.md#存储端口覆盖率报告不并入-just-verify)），不能据此声称数据库覆盖完整。

迁移兼容声明验证在 `crates/auction-storage/tests/postgres/migration_compatibility.rs`，也可用 `just test-db-compat` 单独运行：沿真实迁移链逐级前进，在 V-1 捕获旧程序依赖的契约，执行 V 后要求 `compatible` 登记不收缩任何既有对象、权限或旧版语句结果；`breaking` 只重建基线，另有破坏用例证明比较与写入探针确实会失败。

## 数据库变更

1. 在 [migrations/postgres](../migrations/postgres) 新增迁移文件 `NNNNNN_<名称>.up.sql`（编号连续），见[migrations/postgres/README.md](../migrations/postgres/README.md)。
2. 每次新增迁移都把 [ops/deploy/retention-policy.v2.json](../ops/deploy/retention-policy.v2.json) 的 `database_schema_version` 更新为新的最高版本；新增表还须加入对应数据族。运行态族的新清理规则只能来自用户决策，并须同时加入代码内闭集与政策校验。
3. 同步 [docs/contracts/database.md](contracts/database.md)。

## 接口变更

- 新增或修改路由：同步 `auction-http` 路由表、`auction-contracts` DTO、两端前端与[docs/contracts/routes.md](contracts/routes.md)；新增页面还要修改前端 `Route` 枚举与 [crates/auction-http/src/assets/routes.rs](../crates/auction-http/src/assets/routes.rs)。
- 新增实时事件：更新应用层事件注册表、浏览器解码器与[docs/contracts/realtime.md](contracts/realtime.md)。

## 文档

文档使用中文，Markdown 链接目标按当前文档所在目录计算相对路径，必要时使用 `../`；正文及链接显示文字中的仓库路径仍以仓库根为起点（如 `docs/contracts/routes.md`、`crates/auction-http/src/assets/routes.rs`）。每项事实只在一个 owner 文档中详细维护，其他页面摘要并链接。修改代码时同步相关 README 与契约文档；默认静态门禁离线检查文档相对链接与锚点，移动或重命名时须同步引用。
