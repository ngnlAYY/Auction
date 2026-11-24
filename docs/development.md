# 开发指南

[docs/README.md](README.md) · [docs/architecture.md](architecture.md) · [AGENTS.md](../AGENTS.md) · [ci/README.md](../ci/README.md)

## 环境

| 项 | 要求 |
| --- | --- |
| 平台 | Linux x86_64（原生程序在其他平台编译期报错；浏览器包编译到 `wasm32-unknown-unknown`） |
| Rust | 工具链固定在 [rust-toolchain.toml](../rust-toolchain.toml)（1.100.0，含 clippy、rustfmt 与 wasm32 目标）；MSRV（`Cargo.toml` 的 `rust-version`）与工具链版本相同，不为更旧的版本保留兼容 |
| 工具管理 | [mise](https://mise.jdx.dev) 2026.11.8 及以上：`mise install` 按 [mise.toml](../mise.toml) 与 [mise.lock](../mise.lock) 安装 Rust 之外的全部工具，并读取 `rust-toolchain.toml` 安装 Rust；清单见 [ci/tools/README.md](../ci/tools/README.md) |
| 系统命令 | `bash`、`git`、`perl`（策略门禁需要）、`brotli`（页面构建的预压缩） |
| 数据库 | PostgreSQL 18（运行与运维命令需要；静态检查不需要） |
| 页面构建 | `dx`（Dioxus CLI 0.7）、`wasm-bindgen`、`wasm-opt` 与 `esbuild`，随 `mise install` 安装；`dx` 首次从源码编译，需要 C 编译器、CMake、`pkg-config` 与 OpenSSL 开发文件 |

页面的 TypeScript + SolidJS 实现在 `web/` 下，用同样由 mise 安装的 bun 管理依赖。先运行 `mise run web-prepare` 安装依赖；`bun run typecheck`、`bun run lint` 与包边界检查（TypeScript 语法树）由 `mise run web-check` 运行，并包含在 `mise run verify` 中；行为测试用 `mise run web-test`，不并入默认门禁。完整命令与编码规则见 [web/README.md](../web/README.md)。Public 与 Admin 都已通过各自的 `web-solid` 接入内嵌与分支上的手动候选构建，正式发行仍只产出 Rust 版。

直接调用负责依赖解析的 Cargo 命令时加 `--locked`；mise 任务、CI 检查与发布构建已固定该参数。本地配置命令通过独立参数传递目录，包含空格的路径须在调用 `mise run` 时加引号。如需本机编译加速（sccache、mold 等），只在用户级 `~/.cargo/config.toml` 中配置，不改仓库 profile。

## 常用命令

本文所有命令都在仓库根目录执行，命令中的路径相对于仓库根目录。

```bash
mise install             # 安装或补齐全部工具
mise tasks               # 列出全部入口
mise run verify          # 默认验证：策略门禁 → fmt → check → 依赖版本 → clippy → wasm → rustdoc → web（不运行测试）
mise run test            # 默认 feature 下的纯逻辑、单元与本地替身测试
mise run test-db         # 显式 PG18 集成测试；未设置 AUCTION_TEST_DATABASE_URL 时启动本机临时集群
mise run test-db-compat  # 只验证迁移 compatible 声明（真实 PG18，沿迁移链）
mise run fmt             # 只检查格式
mise run lint            # clippy -D warnings（all-features）
mise run wasm-check      # wasm32 目标 Clippy 检查两个浏览器包（-D warnings）
mise run web-prepare     # 按 web/bun.lock 安装 web/ 依赖（唯一安装 Web 依赖的入口）
mise run web-check       # web/ 的类型检查、Biome 与包边界；已包含在 verify 中
mise run web-test        # web/ 行为测试；不并入默认门禁
mise run web-build       # 构建 Public/Admin 页面 payload（工具随 mise install 安装）
AUCTION_WEB_STACK=solid mise run web-build  # 构建 Solid Public/Admin 页面 payload
mise run web-size        # 只核对两端 Web payload 体积预算（不构建）
mise run run-public      # 按 AUCTION_WEB_STACK（缺省 dioxus）选择 web-embed 或 web-solid
mise run run-admin
mise run run-worker
```

`mise run verify` 等价于 `bash ci/checks/check.sh all fmt check dependency-versions lint wasm rustdoc web`，各项含义见 [ci/README.md](../ci/README.md)。它是纯静态入口：只读源码、配置示例与文档，不安装依赖、不运行 `mise run test`/`mise run test-db`/`mise run web-test`、不连接数据库、不启动服务、不访问网络（`ratchet`、`markdown-links` 等检查均离线）；运行前先 `mise run web-prepare` 准备 `web/node_modules`。`web-dependency-audit`（`bun audit`）不安装依赖，但需要联网读取公告源，不并入 `verify`。因此编译与门禁通过不等于行为已验证，提交前至少通过它。Justfile 中的 `db-migrate`、`db-grants`、`config-check` 等本地命令会实际连接数据库或读取真实配置，不属于静态门禁。

## 本地运行

### 1. 准备数据库

```bash
createuser --pwprompt auction
createdb --owner auction auction
```

或者用维护凭据通过 `auction-cli db create` 建库（见 [apps/auction-cli/README.md#建库](../apps/auction-cli/README.md#建库)）。

### 2. 准备配置

建议把开发配置放在**仓库之外**的私有目录（例如 `~/.config/auction-dev/`），避免误提交敏感文件。加载器、mise 任务与首次设置默认使用的仓库内路径（`ops/config/` 下的非示例文件、`auction-config/`、`*.keyenv`）已被 `.gitignore` 忽略，只是兜底：

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
mise run config-check "$dir"            # 三角色联合检查
mise run config-show worker "$dir"      # 单角色安全投影
```

### 3. 迁移

```bash
mise run db-migrate "$dir" "$owner_secrets"   # auction-cli db migrate --owner-secrets … --no-systemd
mise run db-grants  "$dir" "$owner_secrets"   # 重放运行账号最小权限（迁移 000081）
```

普通启动从不隐式迁移；schema 不是当前版本时进程不会就绪。服务账号是迁移 081 授权的
`auction_runtime`（权限见[docs/contracts/database.md#数据库账号与权限](contracts/database.md#数据库账号与权限)），迁移与恢复必须用 owner-only
的迁移凭据（`--owner-secrets`），本地模板见 `ops/config/examples/secrets/postgres-owner.toml`。

### 4. 启动

```bash
mise run web-build                                        # 首次或前端改动后
AUCTION_CONFIG_FILE=$dir/worker.toml mise run run-worker
AUCTION_CONFIG_FILE=$dir/admin.toml  mise run run-admin
AUCTION_CONFIG_FILE=$dir/public.toml mise run run-public
```

选择 Solid 时先 `mise run web-prepare`，再用 `AUCTION_WEB_STACK=solid mise run web-build` 生成 Public 与 Admin 页面；`run-public` 与 `run-admin` 按同一变量选择各自的 `web-solid`。不设置该变量时仍是 `dioxus`。

默认 Public 监听 `127.0.0.1:8090`、Admin 监听 `127.0.0.1:8091`。未构建页面时页面路由返回 503，API 仍可用。Worker 默认不启用任何外部集成（MoogleMail、论坛、告警），本地开发无需外部凭据；联调 MoogleMail 时可在 `[worker.mooglemail]` 设置 `keyenv_file` 指向本地 `.keyenv`（只接受 `hv-ck-key1`、`hv-ck-key2` 两个键，Worker 使用 `hv-ck-key1`）。

也可以用 `auction-server --config-dir <目录>` 一次启动三个角色（需要启用 Worker control 并允许当前 UID）。

### 本地一口价挂单预览

以下步骤用于 Public、Admin、Worker 和 CLI 均已接入 `consignment-dev-preview` 的集成分支。

1. 新建独立 PG18 集群和空库，仅监听回环地址，按[数据库准备](#1-准备数据库)及[分体部署](../ops/README.md#分体部署)创建运行账号和迁移 owner。按[准备配置](#2-准备配置)复制模板到临时私有目录，三个角色都引用该目录内同一份 `common.toml`。使用临时数据库凭据、Admin 凭据和装备数据密钥，核对运行账号与迁移 owner 都指向这个测试库。禁用邮件、论坛、告警等外部集成。
2. 把公共配置中已有的 `[consignment].enabled` 改为 `true`，配置两个站点的回环监听和对应的 Origin。构建 Solid 页面和四个调试程序，显式给三个服务角色和 CLI 都启用 `consignment-dev-preview`。

```bash
mise run web-prepare
export AUCTION_WEB_STACK=solid
mise run web-build
AUCTION_BUILD_ID="$(bash ci/release/release.sh web-build-id public)"
export AUCTION_BUILD_ID
cargo build --locked -p auction-public-api -p auction-admin-api -p auction-worker -p auction-cli \
  --features 'auction-public-api/web-solid,auction-admin-api/web-solid,auction-public-api/consignment-dev-preview,auction-admin-api/consignment-dev-preview,auction-worker/consignment-dev-preview,auction-cli/consignment-dev-preview'
target/debug/auction-cli --config-dir "$dir" config check --strict
target/debug/auction-cli --config-dir "$dir" db migrate --owner-secrets "$owner_secrets" --no-systemd
```

`reservation_ttl_hours`、`isekai_preclose_days` 等第 4 期字段的默认值、范围、关闭时仍校验的规则与 Credential 数据密钥要求见[寄售服务](../ops/config/README.md#寄售服务)；这里只按该文档填写临时配置。

3. 使用各自的临时配置启动 Worker、Admin、Public，等 Worker 同步寄售准入并发布心跳后检查 Public 的 `/api/v1/consignment/status`。只验证空库和只读 HTTP 时保持队列为空。需要提交登记并观察解析结果时，先在测试装配注入 `ListingEquipmentLookup` 脱敏替身并限制进程出站。若通过 Admin 建立本地测试卖家并签发登录 key，另配独立的测试 `user_login_data_key` 及其标识。

```bash
# 各命令在单独终端运行，先启动 Worker。
AUCTION_CONFIG_FILE="$dir/worker.toml" target/debug/auction-worker
AUCTION_CONFIG_FILE="$dir/admin.toml" target/debug/auction-admin-api
AUCTION_CONFIG_FILE="$dir/public.toml" target/debug/auction-public-api
```

Solid 预览的两个新页面只能经 `AUCTION_WEB_STACK=solid` 构建后访问：Public 用 `/consignment/me/<listing_id>/deposit-instructions` 打开本人寄件指引（未上架或非寄售挂单会按页面状态显示），Admin 用 `/consignment/deposits/<quarantine_id>` 打开邮件隔离详情。Rust 版没有对应页面，同一地址会按未匹配路径回退。这里只说明访问方式，不要求启动服务。

验收停用时先停止三个进程，再把公共开关改回 `false`，从 Worker 开始重新启动。预览数据库及其备份不得作为生产升级输入。

## 代码约定

详见 [AGENTS.md](../AGENTS.md)，要点：

- **目录布局**（`ci/checks/check.sh layout` 强制）：`src/` 根目录只能有 `lib.rs`/`main.rs`，实现放在按职责命名的子目录（不用 common/helpers/shared/misc/utils 兜底）；同一目录不能有 3 个及以上同前缀的源码文件；有子模块的模块以 `name/mod.rs` 为入口，不与同名 `name.rs` 并存；文件与目录名使用 snake_case；每个源文件都必须经 `mod` 声明挂到 crate 根的模块树上；生产源码单文件不超过 600 行，`mod.rs` 不超过 500 行。
- **导入**（`layout` 与 `import-groups` 强制）：生产代码只用显式导入，不写私有 glob（外部 `prelude::*`、单段枚举变体 glob 与 `pub use x::*` 门面除外），每个 `use` 单独成句；子模块从定义处或门面导入，不经 `super::`/`crate::` 路径借用祖先模块的私有 `use` 导入（口径见[ci/checks/README.md#模块层级门禁](../ci/checks/README.md#模块层级门禁)）；模块级私有 `use` 只写在模块开头的导入区，整个导入区按 std → 外部 crate → 本 crate 三组排列、组间空一行。
- **依赖方向**：domain ← application ← 适配器 ← apps；前端只依赖 contracts 与 web-shared。
- **命令**：写操作带稳定 `command_id`、请求摘要与期望版本，在一个事务内写入事实、结果、审计与事件。
- **注释与文档**：中文；每个非测试源码文件以 `//!` 说明模块职责（`ci/checks/check.sh module-docs` 强制），公共项用 `///`，复杂分支说明原因、不变量与取舍。
- **配置示例**：[ops/config/examples](../ops/config/examples) 中每个赋值字段前必须紧邻含“可选”或“必填”的中文注释（`ci/checks/check.sh config`）。示例同时是首次设置生成器的模板，修改后须保证生成的配置仍能通过 loader 校验。
- **前端文案**：全部走 `i18n/*.ftl`，五种语言键集合一致，代码引用的键必须存在于 `en.ftl`，键名不得含点号；其他语言只能引用英文译文已有的变量，字面量键的 `t!` 调用必须传入英文译文引用的全部变量（翻译失败时 `t!` 在开发与测试构建中 panic，发布构建退回显示消息 ID 并上报一次 `i18n_format` 信标）（`ci/checks/check.sh i18n`）。
- **装备诊断闭集**：装备解析诊断原因在 Rust、SQL 约束、Admin 文案之间必须一致（`ci/checks/check.sh equipment-contract`）。
- **源码棘轮与文档链接**：`mise run verify` 包含 `ratchet` 与 `markdown-links`；模式计数只能下降，基线须随清理改动更新；相对链接及同页、跨页锚点必须有效，表格之后须空一行，口径见[ci/checks/README.md](../ci/checks/README.md)。

## 测试

`mise run verify` 只做格式、编译与静态分析；测试使用独立入口，不并入默认门禁。行为变更必须随功能维护并运行受影响范围的测试，至少覆盖正反例与所修复问题的回归断言；已批准的测试类别与其他行为验证的授权范围见 [AGENTS.md#验证范围](../AGENTS.md#验证范围)。

```bash
mise run test        # cargo test --locked --workspace --all-targets（默认 feature）
mise run test-wasm   # Firefox 中的 WASM 收包接线测试
mise run test-db     # 真实 PostgreSQL 18 集成测试
```

门禁脚本的回归用例用 `mise run gate-self-test` 离线运行。工具与依赖准备见[门禁自测](../ci/checks/README.md#门禁自测)。

`mise run test` 运行单元测试及无需数据库的集成测试（包括本地替身）；不运行文档测试，不要求 PostgreSQL，不连接真实游戏或论坛账号，也不得隐式启动数据库集群。可按改动所属 owner 缩小范围，例如 `cargo test --locked -p auction-domain --lib`；交付说明写出实际命令及未覆盖范围。

`mise run test-wasm` 需要本机已安装 Firefox，由 mise 提供的 `wasm-bindgen-test-runner` 和 `geckodriver` 以无头模式运行。入口依次运行 `auction-web-shared` 的 `transport_browser` 集成测试和 `auction-web-public` 的 `api::conditional_get_tests` 模块。前者验证共享传输的状态、响应头与正文读取顺序，后者验证 Public 拍品详情的条件 GET 和缓存驱逐。Fetch 替身提供浏览器的 `Response`，不连接应用或外部账号。日常运行 `mise run test` 无需浏览器。

`mise run test-db` 启用 `auction-storage` 的 `test-db` feature，断言涉及锁、隔离级别、幂等或迁移时必须在真实 PG18 上执行。`crates/auction-storage/src` 内另有少量以 `postgres_` 命名、标注 `#[ignore]` 的用例：它们只在事务内写连接私有临时表，设置了 `AUCTION_TEST_DATABASE_URL` 时由同一入口追加运行，未设置时明确跳过。设置 `AUCTION_TEST_DATABASE_URL` 时复用该 URL 所在服务器，只创建与删除夹具自己的模板库和测试库，不改写 URL 指向的库。该维护角色需要 `CREATEDB` 与 `CREATEROLE`（CI 显式授予，生产运行账号不授予）。外部数据包装器盘点回归使用一次性本机集群，因为 PostgreSQL 只允许 superuser 创建 foreign-data wrapper。这个用例及未设置 URL 时的整套测试依次从 `AUCTION_TEST_PG_BIN`、`PATH`、`/usr/lib/postgresql/18/bin` 查找 PG18 二进制。URL 只经环境传入，不写入文档、日志或提交的配置。该入口不得并入 `mise run verify` 或 `mise run test`：需要真实 PostgreSQL 语义的断言只能在这里运行，并保持独立数据库、独立角色与清理后的临时集群。

夹具先在空库上执行全部内嵌迁移得到模板库，再按测试 `CREATE DATABASE ... TEMPLATE` 克隆；析构时 `DROP DATABASE ... WITH (FORCE)`，临时集群在最后一个测试释放句柄后停止并删除数据目录。`postgres_` 前缀是数据库测试的命名选择契约。夹具另建只属于 `auction_runtime` 的临时登录角色，`TestDatabase::store()` 默认以它连接，用于证明最小权限下事务可以提交；`maintenance_store()` 只用于迁移、DDL 夹具与必须越过运行账号权限的断言读取，以维护账号驱动被测端口会掩盖运行账号缺权。

并行复用同一外部 PG18 测试服务器时，各进程使用同一维护数据库。夹具在该库持有会话级 advisory lock，串行化运行角色初始化和失败清理。
非 superuser 维护账号继承夹具新建的临时登录角色，以便结束该角色的测试连接。清理时随临时角色一起撤销该成员资格。

会写入进程级审计健康计数的测试放在 `postgres_audit` 目标里。`mise run test-db` 在 `postgres` 之后运行该目标，两者各用一个测试进程。寄售源码内的场景分派必须先验证当前目标，再执行会记录审计失败的场景，避免影响 `postgres` 中的就绪检查。

寄售第 4 期的 PG 用例在 `crates/auction-storage/tests/postgres/consignment/deposit/`，可按过滤词单独运行：`cargo test --locked -p auction-storage --features test-db --test postgres consignment::deposit`。会改动进程级审计健康计数的寄售场景在 `postgres_audit` 目标（`crates/auction-storage/tests/postgres_audit/consignment/`），例如 `cargo test --locked -p auction-storage --features test-db --test postgres_audit postgres_deposit_audit_failure_rolls_back_multi_listing_command`；这些用例只由 `mise run test-db` 或显式入口运行，不并入默认门禁。

同一入口接着用 `cargo build --locked -p auction-cli` 构建默认调试二进制，再通过 `--ignored consignment::key_rotation::postgres_cli_` 运行 `postgres` 目标中的装备数据密钥轮换 CLI 用例。这一组用真实子进程和夹具创建的独立数据库，覆盖演练、实际重封和旧钥退休检查。CI 的 `test` 作业调用这一入口。

本地替身由 `crates/auction-test-support` 提供，只绑定 IPv4 回环、以脱敏语料回放，绝不连接真实账号；`crates/auction-integrations/tests/` 的邮件与论坛替身用真实客户端覆盖读取、账号忙、Cookie 失效、超时、协议漂移与写入结果分类，`mooglemail_maintenance_contention.rs` 复现“完整观察与 Sent 维护共用请求门导致维护超时”，`returned_take_timing.rs` 用真实投递 transport 复现结算退件 Take 的旧 POST 迟到完成与重排后新预检的两种交错（远端附件至多取走一次，已取走时新预检失败关闭、不再发 POST），以及领取 POST 已发出后被投递 Worker 放弃时，重排后的新预检等远端回应后才读取、不与之重叠。`crates/auction-integrations/src/clients/mooglemail/sender/baseline/tests.rs` 把 Sent 页数上限设为 1，覆盖发送基线在 `full`/`shadow` 下的失败关闭、`watermark` 的边界取得与写后候选；`apps/auction-worker/src/bootstrap/shutdown/tests.rs` 在发送 POST 中途停止 Worker，验证结果落库后才退出、准备期工作立即放弃、预算耗尽只留 sending 围栏且不重发。`crates/auction-integrations/src/equipment/reader/tests.rs` 用注入的账号状态故障覆盖装备页读取与邮件请求共用同账号许可：战斗页与限流只延期不退件、熔断窗口内不发网络、物理请求不重叠（装备页本身没有语料），以及装备读取被放弃时撤销许可登记、已发出时持锁到远端回应或本次读取预算结束。`crates/auction-integrations/src/clients/mooglemail/client/request/tests.rs` 用不随断开停止处理的 `DetachedDelay` 覆盖 Worker 自身读取与写入 POST 被放弃的两段：排队中放弃不留票据与锁登记、远端收不到；已发出后被调用方或余额读取预算放弃时，紧接的请求等远端处理完前一请求，远端处理超过 HTTP 预算时在预算结束释放，被放弃的写入只发出一次；未放弃的读写节奏与写入回应不变。替身未采集的远端页面、Worker 完整投递 runtime 与论坛写入成功页仍无替身驱动的用例；存储端口方法覆盖率用 `bash ci/checks/check.sh port-coverage` 统计，该报告不并入 `mise run verify`（口径见[ci/checks/README.md#存储端口覆盖率报告不并入-mise-run-verify](../ci/checks/README.md#存储端口覆盖率报告不并入-mise-run-verify)），不能据此声称数据库覆盖完整。

迁移兼容声明验证在 `crates/auction-storage/tests/postgres/migration_compatibility.rs`，也可用 `mise run test-db-compat` 单独运行：沿真实迁移链逐级前进，在 V-1 捕获旧程序依赖的契约，执行 V 后要求 `compatible` 登记不收缩任何既有对象、权限或旧版语句结果；`breaking` 只重建基线，另有破坏用例证明比较与写入探针确实会失败。

## 数据库变更

1. 在 [migrations/postgres](../migrations/postgres) 新增迁移文件 `NNNNNN_<名称>.up.sql`（编号连续），见[migrations/postgres/README.md](../migrations/postgres/README.md)。
2. 每次新增迁移都把 [ops/deploy/retention-policy.v2.json](../ops/deploy/retention-policy.v2.json) 的 `database_schema_version` 更新为新的最高版本；新增表还须加入对应数据族。运行态族的新清理规则只能来自用户决策，并须同时加入代码内闭集与政策校验。
3. 同步 [docs/contracts/database.md](contracts/database.md)。

## 接口变更

- 新增或修改路由：同步 `auction-http` 路由表、`auction-contracts` DTO、两端前端与[docs/contracts/routes.md](contracts/routes.md)；新增页面还要修改前端 `Route` 枚举与 [crates/auction-http/src/assets/routes.rs](../crates/auction-http/src/assets/routes.rs)。
- 新增实时事件：更新应用层事件注册表、浏览器解码器与[docs/contracts/realtime.md](contracts/realtime.md)。

## 文档

文档使用中文，Markdown 链接目标按当前文档所在目录计算相对路径，必要时使用 `../`；正文及链接显示文字中的仓库路径仍以仓库根为起点（如 `docs/contracts/routes.md`、`crates/auction-http/src/assets/routes.rs`）。每项事实只在一个 owner 文档中详细维护，其他页面摘要并链接。修改代码时同步相关 README 与契约文档；默认静态门禁离线检查文档相对链接与锚点，移动或重命名时须同步引用。
