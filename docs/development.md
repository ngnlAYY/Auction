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
just verify          # 默认验证：策略门禁 → fmt → check → feature → clippy → wasm（不运行测试）
just test            # 默认 feature 下的纯逻辑、单元与本地替身测试
just test-db         # 显式 PG18 集成测试，要求 AUCTION_TEST_DATABASE_URL
just test-db-compat  # 只验证迁移 compatible 声明（真实 PG18，沿迁移链）
just fmt             # 只检查格式
just lint            # clippy -D warnings（all-features）
just wasm-check      # wasm32 目标 Clippy 检查两个浏览器包（-D warnings）
just web-build       # 构建 Public/Admin 页面 payload（需要受控 dx）
just web-size        # 只核对两端 Web payload 体积预算（不构建）
just run-public      # cargo run --locked -p auction-public-api --features postgres,web-embed
just run-admin
just run-worker
```

`just verify` 等价于 `bash ci/checks/check.sh all fmt check feature dependency-versions lint wasm`，各项含义见 [ci/README.md](../ci/README.md)。提交前至少通过它。

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

也可以选择首次设置向导生成整套配置（需要 `postgres,web-embed` 构建），替代上面的手工创建步骤：先令 `dir` 指向一个尚不存在的目录，并预先建立符合[ops/config/README.md#示例与首次设置](../ops/config/README.md#示例与首次设置)的直接父目录，再运行 `cargo run --locked -p auction-admin-api --features postgres,web-embed -- setup --output-dir "$dir"`。

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

- **目录布局**（`ci/checks/check.sh layout` 强制）：`src/` 根目录只能有 `lib.rs`/`main.rs`，实现放在按职责命名的子目录；同一目录不能有 3 个及以上同前缀的源码文件；文件与目录名使用 snake_case。
- **依赖方向**：domain ← application ← 适配器 ← apps；前端只依赖 contracts 与 web-shared。
- **命令**：写操作带稳定 `command_id`、请求摘要与期望版本，在一个事务内写入事实、结果、审计与事件。
- **注释与文档**：中文；模块用 `//!`，公共项用 `///`，复杂分支说明原因、不变量与取舍。
- **配置示例**：[ops/config/examples](../ops/config/examples) 中每个赋值字段前必须紧邻含“可选”或“必填”的中文注释（`ci/checks/check.sh config`）。示例同时是首次设置生成器的模板，修改后须保证生成的配置仍能通过 loader 校验。
- **前端文案**：全部走 `i18n/*.ftl`，五种语言键集合一致，代码引用的键必须存在于 `en.ftl`，键名不得含点号（`ci/checks/check.sh i18n`）。
- **装备诊断闭集**：装备解析诊断原因在 Rust、SQL 约束、Admin 文案之间必须一致（`ci/checks/check.sh equipment-contract`）。
- **源码棘轮与文档链接**：`just verify` 包含 `ratchet` 与 `markdown-links`；模式计数只能下降，基线须随清理改动更新；相对链接及同页、跨页锚点必须有效，口径见[ci/checks/README.md](../ci/checks/README.md)。

## 测试

`just verify` 继续只做格式、编译与静态分析；测试使用独立入口，不并入默认门禁。行为变更必须随功能维护并运行受影响范围的测试，至少覆盖正反例与所修复问题的回归断言。已批准的 W6 类别包括纯逻辑、属性/快照、脱敏页面回放、PostgreSQL 18 集成，以及本地 MoogleMail/论坛替身；其他行为验证仍按 [AGENTS.md#验证范围](../AGENTS.md#验证范围) 的授权范围执行。

```bash
just test
# 等价于：cargo test --locked --workspace --all-targets

just test-db
# 未设置 AUCTION_TEST_DATABASE_URL 时用本机 PG18 二进制启动临时集群，再执行：
# cargo test --locked -p auction-storage --features test-db --test postgres -- postgres_
```

`just test` 使用默认 feature，运行单元测试及无需数据库的集成测试（包括本地替身）；不运行文档测试，不要求 PostgreSQL，不连接真实游戏或论坛账号。可按改动所属 owner 缩小范围，例如 `cargo test --locked -p auction-domain --lib`；交付说明写出实际命令及未覆盖范围。

本地替身（W6.6）由 `crates/auction-test-support` 提供，只绑定 IPv4 回环、以 W6.3 脱敏语料回放，绝不连接真实账号。`crates/auction-integrations/tests/` 的 `mooglemail_double.rs` 与 `forum_double.rs` 用真实客户端覆盖读取、账号忙、Cookie 失效、超时、协议漂移与写入结果分类；`mooglemail_maintenance_contention.rs` 复现 D2：Read Mail 完整观察与 Sent 维护共用请求门时维护在本轮预算内超时，维护单独先跑时同预算内完成。`crates/auction-integrations/src/clients/mooglemail/sender/baseline/tests.rs` 把 Sent 页数上限设为 1（main 第 0 页带有未采集的下一页）覆盖 W1.4 发送基线：`full`/`shadow` 在发送前以页数超限失败关闭（D3），`watermark` 只读第 0 页取得边界，锚点失效或每日完整校验超限都不阻断发送，写后只把边界之上的新邮件当作候选，读不到 Sent 时失败关闭；已采集的 Sent 详情都带附件，完整 `send()` 成功证明不在替身上回放。`apps/auction-worker/src/bootstrap/shutdown/tests.rs` 在发送 POST 中途停止 Worker，验证 W7.11 关键段排空：结果落库后才退出、准备期工作立即放弃、预算耗尽只留下 sending 围栏且不重发，`critical_write_timeout_seconds = 0` 的旧路径复现 D8。覆盖限制：Worker 循环按 `poll_isolation_enabled` 选择调度形状的代码与完整投递 runtime 没有替身驱动的用例；替身没有论坛 PM/回帖写入成功页；main 世界 Sent 与 Read Mail 只采集了前 1 页与前 3 页，需要完整枚举的测试须显式调用 `set_captured_pages_complete`。

`just test-db` 启用 `auction-storage` 的 `test-db` feature。设置 `AUCTION_TEST_DATABASE_URL` 时复用该 URL 所在服务器，只创建与删除夹具自己的模板库和测试库，不改写 URL 指向的库；该维护角色需要 `CREATEDB` 与 `CREATEROLE`（CI 已显式授予，生产运行账号不授予）；未设置时依次用 `AUCTION_TEST_PG_BIN`、`PATH`、`/usr/lib/postgresql/18/bin` 的 PostgreSQL 18 二进制在临时目录启动一次性集群。URL 只通过环境传入，不写入文档、日志或提交的配置；夹具校验服务器 major 为 18。涉及锁、隔离级别、幂等或迁移的断言必须在真实 PG18 上执行。

夹具先在新建空库上执行全部内嵌迁移得到模板库，再由每个测试 `CREATE DATABASE ... TEMPLATE` 克隆独立库；测试库析构时执行 `DROP DATABASE ... WITH (FORCE)`，临时集群在最后一个测试释放共享句柄后停止服务并删除数据目录。`postgres_` 前缀是数据库测试的命名选择契约。

夹具还为每个集群建立一个一次性登录角色：只是 `auction_runtime` 的成员、继承其授权，没有 superuser/CREATEDB/CREATEROLE，也不拥有对象，随机密码只在进程内存中，集群释放时删除（外部服务器上要求维护角色对 `auction_runtime` 有 ADMIN OPTION，由它创建该角色时自动具备）。`TestDatabase::store()` 默认以该角色连接，被测端口都经它执行，用于证明事务在 W9.9 生产运行账号的最小权限下可以提交；`TestDatabase::maintenance_store()` 以维护账号连接，只用于迁移、DDL 夹具（删表、建触发器、重放授权函数）与必须越过运行账号权限的断言读取。以维护账号驱动被测端口会掩盖运行账号缺权（例如对只追加表加行锁所需的 `UPDATE` 权限），因此不要这样写。

迁移兼容声明验证（W7.8）在 `crates/auction-storage/tests/postgres/migration_compatibility.rs`，也可用 `just test-db-compat` 单独运行。它从 075 起沿真实迁移链逐级前进，在 V-1 捕获旧程序依赖的契约（关系、列、默认值、约束、触发器、函数、域、枚举、权限），执行 V 后要求 `compatible` 登记不收缩任何既有对象、权限或旧版语句结果；`breaking` 只重建基线。另一个人为破坏用例证明同一比较与写入探针确实会失败。夹具使用 `auction_storage::postgres_migrate_to` 复用正式迁移执行视图，只在 `test-db` feature 下可用。

当前 `crates/auction-storage/tests/postgres/` 已覆盖空库迁移、模板克隆、竞价队列与跨 lot 并发、lot watch outbox、realtime 序号、告警闭集、保留清理、快照时区、投递处置，以及以运行账号执行的结算端到端（领取 → 结算 → 重放 → 完成）、付款邮件未证实寄出时的恢复推进延后（W14.6）与人工 Sent 证明的拒绝分支；运行账号不能执行 DDL/`TRUNCATE`、不能改写只追加表（W9.9），`rekey` 后旧 key 无法登录、原有会话全部失效（W9.10），停机时段门禁只保护 Final 场次与截止落在窗口内（含已过截止）的 live lot（W7.5），共约 90 个用例。人工 Sent 证明的准入成功分支需要完整的发送尝试、凭据与资金事实，尚无夹具；存储端口方法覆盖率可用 `bash ci/checks/check.sh port-coverage` 统计（2026-11-09 为 244 个方法中的 56 个，约 23%，口径见[ci/checks/README.md#存储端口覆盖率报告不并入-just-verify](../ci/checks/README.md#存储端口覆盖率报告不并入-just-verify)），不能据此声称数据库覆盖完整。数据库测试只在 `just test-db` 运行，默认 `just test` 不要求数据库；后续按 owner 扩展时保持同一入口、临时集群与逐测试独立库。

## 数据库变更

1. 在 [migrations/postgres](../migrations/postgres) 新增迁移文件 `NNNNNN_<名称>.up.sql`（编号连续），见[migrations/postgres/README.md](../migrations/postgres/README.md)。
2. 每次新增迁移都把 [ops/deploy/retention-policy.v2.json](../ops/deploy/retention-policy.v2.json) 的 `database_schema_version` 更新为新的最高版本；新增表还须加入对应数据族。运行态族的新清理规则只能来自用户决策，并须同时加入代码内闭集与政策校验。
3. 同步 [docs/contracts/database.md](contracts/database.md)。

## 接口变更

- 新增或修改路由：同步 `auction-http` 路由表、`auction-contracts` DTO、两端前端与[docs/contracts/routes.md](contracts/routes.md)；新增页面还要修改前端 `Route` 枚举与 [crates/auction-http/src/assets/routes.rs](../crates/auction-http/src/assets/routes.rs)。
- 新增实时事件：更新应用层事件注册表、浏览器解码器与[docs/contracts/realtime.md](contracts/realtime.md)。

## 文档

文档使用中文，Markdown 链接目标按当前文档所在目录计算相对路径，必要时使用 `../`；正文及链接显示文字中的仓库路径仍以仓库根为起点（如 `docs/contracts/routes.md`、`crates/auction-http/src/assets/routes.rs`）。每项事实只在一个 owner 文档中详细维护，其他页面摘要并链接。修改代码时同步相关 README 与契约文档；默认静态门禁离线检查文档相对链接与锚点，移动或重命名时须同步引用。
