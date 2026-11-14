# AGENTS.md — HV Auction 协作规范

本文件约束所有贡献者与自动化代理，是模块边界、领域不变量、代码/配置/迁移/文档规则与交付流程的唯一来源。业务、接口、数据与部署语义由本仓库独立定义，实现、构建与验收不依赖外部源码仓库。事实细节由下表的 owner 文档维护，本文只写规则并链接。

| 想了解 | 去读 |
| --- | --- |
| 进程模型、分层与 crate 职责、共享 helper 归属、一致性模型 | [docs/architecture.md](docs/architecture.md) |
| 场次、竞价、结算、邮件与论坛业务规则 | [docs/business-rules.md](docs/business-rules.md)、[docs/guide](docs/guide) |
| 环境、常用命令、本地运行与测试细节 | [docs/development.md](docs/development.md) |
| 每项静态门禁的口径、CI 作业与发布构建 | [ci/README.md](ci/README.md)、[ci/checks/README.md](ci/checks/README.md) |
| 路由、实时事件与数据库契约，迁移规则 | [docs/contracts](docs/contracts)、[migrations/postgres/README.md](migrations/postgres/README.md) |
| 配置字段与示例 | [ops/config/README.md](ops/config/README.md) |
| 部署、备份、监控与运维手册 | [ops/README.md](ops/README.md) |
| 单个程序或 crate 的细节 | 各自的 `apps/*/README.md`、`crates/*/README.md` |

## 项目速览

- HentaiVerse 装备拍卖系统：玩家经网站与论坛出价，经游戏内 MoogleMail 寄售装备、付款与收货。
- `apps/` 下五个可部署程序：`auction-public-api`（Public 竞价站点）、`auction-admin-api`（Admin 后台与分体部署首次设置）、`auction-worker`（生命周期、结算履约、MoogleMail、论坛、告警等全部后台任务）、`auction-cli`（配置检查、部署编排、迁移、备份恢复与业务运维），以及可选的 `auction-server`（单主机上以子进程监管前三个角色）。
- Rust 2024 工作区：工具链固定为 1.99.0（`rust-toolchain.toml`），MSRV 1.94.1 由 CI 单独检查。后端用 axum 与 sqlx；前端是两个 Dioxus 0.7 CSR 单页应用，编译到 `wasm32-unknown-unknown`，由后端以 `web-embed` 内嵌。
- 原生程序只支持 Linux x86_64，装配根在其他平台编译期报错；通用制品不使用 `target-cpu=native`。
- PostgreSQL 18 是唯一数据库后端与唯一事实来源，没有独立缓存进程。未配置数据库时进程以运行期骨架模式启动，相关接口返回“能力不可用”；不存在“无数据库”的编译形态。
- 普通启动从不隐式迁移。迁移只经 `auction-cli db migrate`、`auction-cli service start/restart`（停机门禁后）与合并部署首次初始化（网页明确确认后的专用空库）执行。

## 依赖方向与模块边界

依赖方向向内：`apps/*` 装配根注入适配器，适配器实现 `auction-application` 的窄端口，应用层只依赖领域。分层图与各 crate 的职责说明见[docs/architecture.md#分层](docs/architecture.md#分层)；下表只列各模块不得越过的边界。

| 模块 | 边界 |
| --- | --- |
| `auction-domain` | 纯领域规则；不导入 axum、dioxus、sqlx、reqwest，不读环境、不读系统时钟、不初始化日志、不访问外部服务 |
| `auction-application` | 用例、事务编排、应用错误与窄端口；不读进程配置、不监听、不直接执行 SQL 或外部调用 |
| `auction-contracts` | 前后端共用的稳定 DTO；数据库记录不能直接成为浏览器 DTO |
| `auction-storage` | 实现应用端口，拥有全部 SQL、连接池、事务与锁；业务决定留在内层 |
| `auction-integrations` | 外部传输与认证；响应经应用命令或持久化 Worker 处理，不直接改拍卖状态 |
| `auction-http` | 输入/命令与输出/DTO 映射；不包含竞价、结算或状态机规则 |
| `auction-jobs` | 调度、重试、租约与 Worker 编排；不建立第二套状态机 |
| `auction-config` | TOML 解析与校验；业务模块不得散落环境变量读取 |
| `auction-setup` | 首次设置、一次性令牌与初始化门禁；正式 Admin 不挂载设置写接口 |
| `auction-observability` | 日志、指标、tracing、心跳；敏感值不得进入日志 |
| `auction-process` | 信号、有界排空、任务守卫、本机 readiness 探针；不读配置、不监听 |
| `auction-test-support` | 只进 `dev-dependencies` 的回环外部服务替身与脱敏语料；不连接真实账号 |
| `auction-web-public` / `auction-web-admin` | 各自的页面、路由、交互与译文；互不依赖，不导入任何后端 crate |
| `auction-web-shared` | 两端通用、无业务偏向的浏览器能力与设计 token |
| `apps/*` | 参数与配置加载、依赖注入、监听绑定、优雅关闭与退出码 |

- 功能按“领域 → 用例/端口 → 存储/外部适配 → HTTP/Web/Worker”的顺序落地。
- 每个表、迁移、查询、事件、路由与配置字段只有一个 owner；历史格式经映射层兼容，不把旧命名传播到新领域。
- 跨装配根的共享机制各有唯一 owner：HTTP listener 与连接服务、附加指标出口、tracing 收口、本机 readiness 探针、CLI 私有文件与子进程边界，以及浏览器的 `wire`/`storage`/`timer`。归属与调用顺序见[docs/architecture.md#共享-helper-归属](docs/architecture.md#共享-helper-归属)。`apps/*` 不复制第二份实现。

## 领域不变量

修改写入、状态迁移、结算或后台任务时，必须在代码注释或提交说明中写明所保护的不变量。

1. 可变命令使用稳定 `command_id`：同 ID 同请求返回原结果，同 ID 不同请求冲突。
2. 在同一事务内锁定拍卖、lot 与有效授权，校验资格与状态并计算结果。竞价与关键生命周期使用 Serializable 隔离与明确行锁，序列化冲突与死锁有界重试；禁止跨事务“先读价后写入”。
3. 被接受的竞价事实、版本、追加审计与持久事件必须同事务写入。
4. 论坛、邮件等外部写入只在本地提交后执行；外部失败不能抹掉已接受的竞价。外部结果未知时转人工对账，绝不自动重发。
5. Worker 的工作项、游标与命令账本必须持久化；内存定时器只能用于唤醒，不能作为到期、租约或完成状态的来源。
6. 非法状态迁移、金额、编码、身份或过期版本一律失败关闭，不静默修正。
7. 会话凭据、外部 Cookie、登录 key、数据密钥等秘密不得出现在日志、DTO、快照或提交的配置中。
8. 审计只追加；更正形成新事实，不编辑历史证据。
9. 缓存不得决定金额、资格、幂等、锁、会话或审计；禁用或丢失缓存不影响正确性。
10. 已冻结的场次策略（经济、结算、恢复）永不改写；改变计算结果必须以新的策略版本表达。

## 源码结构

以下规则由 `ci/checks/check.sh` 的 `layout`、`import-groups`、`module-docs`、`ratchet` 等门禁强制，口径见[ci/README.md](ci/README.md)。

- `apps/*/src`、`crates/*/src` 根目录只放薄 `main.rs`/`lib.rs`：模块声明、重导出、参数装配与进程生命周期。业务规则、SQL、HTTP 处理器、组件与外部调用放进按单一职责命名的子目录，即使只有一个文件。
- 目录与文件使用明确的职责名（如 `ports/`、`services/`、`errors.rs`、`labels.rs`），不用 `common`、`helpers`、`shared`、`misc`、`utils` 兜底；一律 snake_case。
- 有子模块的模块以 `name/mod.rs` 为入口，叶子模块是单个 `name.rs`，两者不得并存。同一目录出现 3 个及以上同前缀的源码文件，说明缺少子目录。模块树由目录层级表达，不用 `#[path]`。
- 生产源码单文件不超过 600 行，`mod.rs` 不超过 500 行，从文件开头数到首个行首 `#[cfg(test)]`；超过时按职责拆成子模块。
- 生产代码只用显式导入，从定义处或门面导入所需名字：不写私有 glob（`use super::*`、`use mapping::*`、`use self::别名::*` 等），父模块不为子模块囤积导入。只放行外部 `prelude::*`、函数内枚举变体 glob 与 `pub use x::*` 门面；测试模块可用 `use super::*`。
- 每个 `use` 单独成句，不写 `use { … };`。导入块按 std → 外部 crate（含工作区其他 crate）→ 本 crate 三组排列，组间空一行。
- 吞掉错误的写法（`Err(_)`、`map_err(|_| …)`、`let _ = error;`）和测试模块的 `use super::*` 受源码计数棘轮约束，只能减少；清理后按[ci/checks/README.md#源码计数棘轮](ci/checks/README.md#源码计数棘轮)下调基线，不得为放行新代码上调。

## 代码风格

- 使用仓库 rustfmt。Clippy 以 `-D warnings` 运行；workspace lint 把 `unwrap_used`、`expect_used`、`dbg_macro`、`todo`、`unimplemented` 设为 deny，测试豁免集中在 `clippy.toml`。不用关闭 lint 掩盖设计问题，必要的例外只加在单个条目上并用中文写明原因。
- 函数单一职责，优先早返回与明确错误；库用 `thiserror`，只有装配根使用 `anyhow`。生产代码显式传播错误或处理 `None`；不可恢复的启动不变量要给出可行动的错误。
- 使用结构化 tracing 字段；完整凭据、密码、Cookie 与密钥不得进入日志或错误信息。
- DTO、数据库记录与领域聚合分离，在边界映射。
- 只做需求相关的最小修改，不顺手重构或改名；新增 `pub` 接口前确认有调用方。
- 命名：模块、文件、函数、变量、字段用 `snake_case`；类型、trait 与 Dioxus 组件用 `PascalCase`；常量用 `SCREAMING_SNAKE_CASE`；缩写按 Rust 风格（`HttpClient`、`ApiError`）。孤立的业务标识带上下文（`auction_id`、`lot_id`、`command_id`），避免含混的 `id`/`data`/`item`；类型已提供上下文的字段（如 `Auction.id`）与已发布的路径、事件名和字段按契约保留。新 URL 用小写连字符；JSON、TOML、SQL 名用 `snake_case`。
- CSS 两空格缩进，类名用小写连字符与 BEM，选择器带 surface 前缀（如 `public-auction-catalog__name`、`admin-`）；颜色、字号、间距使用 [crates/auction-web-shared/style/base.css](crates/auction-web-shared/style/base.css) 的共享变量，surface 专属规则留在 `crates/auction-web-public/assets/` 与 `crates/auction-web-admin/assets/`。

## 注释与文档

- 注释与文档使用中文；协议、API、SQL 关键字与必须匹配的字符串保留原文。
- 每个非测试源码文件以 `//!` 说明模块职责；公共类型、函数与端口用 `///`；复杂分支用 `//` 解释原因、不变量、锁/幂等、兼容、安全或性能取舍。
- 注释只写代码本地的不变量与理由，设计背景交给 owner 文档。不复述代码，不把字段名直译一遍，删除过期注释。文档注释里的符号链接必须能被 rustdoc 解析（`rustdoc` 门禁以 `-D rustdoc::all` 构建）。
- 每项事实只在一个 owner 文档中详细描述，其他地方摘要并链接。路由、状态码、字段、事件、配置、迁移或制品变化时，同步对应 README 与契约文档：[docs/contracts/routes.md](docs/contracts/routes.md)、[docs/contracts/realtime.md](docs/contracts/realtime.md)、[docs/contracts/database.md](docs/contracts/database.md)、[ops/config/README.md](ops/config/README.md)。
- Markdown 链接目标按当前文档所在目录计算相对路径，必要时用 `../`；同页锚点写 `#标题锚点`。正文与链接显示文字中的仓库路径以仓库根为起点（如 `docs/contracts/routes.md`）。`markdown-links` 门禁离线检查相对链接与锚点，移动、重命名文件或标题时须同步引用。

## 配置

权威说明见[ops/config/README.md](ops/config/README.md)。

- 运行配置一律 TOML；`AUCTION_CONFIG_FILE` 只选择角色文件，不覆盖任何字段；不引入通用环境变量覆盖。
- 三角色必须引用同一 `common_file`；`runtime_config_file` 要么三者引用同一个，要么都省略；数据库目标必须相同。
- 数据库、Cookie、Admin 凭据、数据密钥只进敏感 TOML。敏感文件不提交，只允许运行账号读取；错误信息不得回显 TOML 原文或秘密值。
- `ops/config/examples/` 中每个赋值字段前必须紧邻含“可选”或“必填”的中文注释，说明默认值与启用条件（`config` 门禁）。示例同时是首次设置的生成模板，修改后生成的配置仍须通过 loader 校验。
- 每种外部 Cookie（邮件、论坛）全库同时最多一个 active。

## 数据库与迁移

- PostgreSQL 固定 major 18；驱动随 `auction-storage` 无条件编译，不设后端 feature，不使用 `AnyPool`。
- 迁移只追加：已发布 tag 中的迁移文件逐字节不可修改、重命名或删除（`published-migrations`）；075 起每个迁移末尾登记自身的兼容等级（`migration-compatibility`）。规则见[migrations/postgres/README.md](migrations/postgres/README.md)。
- 每次新增迁移都把 [ops/deploy/retention-policy.v2.json](ops/deploy/retention-policy.v2.json) 的 `database_schema_version` 更新为新的最高版本；新表还要加入对应数据族，并同步[docs/contracts/database.md](docs/contracts/database.md)。
- 服务以最小权限的 `auction_runtime` 运行（无 DDL、无 `TRUNCATE`、只追加表仅 `INSERT`）；迁移与恢复使用 owner-only 的迁移凭据。
- 破坏性数据库操作只能经 `auction-cli` 的显式确认、可验证备份与回执完成；开发脚本不得加入 `DROP DATABASE`。

## Web

- Public/Admin 是独立的 Dioxus CSR SPA，各自拥有路由、译文与资源。不启用 SSR、hydration 或 fullstack，不引入 JavaScript 框架、TypeScript 或 npm 运行时依赖；`dx` 构建时使用的 esbuild 是唯一例外。
- `just web-build` 用 `dx bundle` 以 `wasm-release` profile 构建，复制到稳定目录后由后端 `web-embed` 内嵌。未构建时后端仍可编译，页面路由返回 503。只有显式 `web-embed` 才内嵌资源；Public 二进制不得包含 Admin 资源，反之亦然。
- 浏览器只消费 `auction-contracts` DTO；改变状态的动作携带 `command_id`，结果未知时只允许原样重试。不在浏览器重建业务状态机。
- 所有界面文案走各自的 `i18n/*.ftl`：五种语言（en、zh-CN、zh-TW、ja、ko）键集合一致，键名用连字符、不含点号（`i18n` 门禁）。Public 默认 en，Admin 默认 zh-CN；偏好键 `hv.lang`、`hv.theme` 是固定契约。
- 新增或删除页面时，同步前端 `Route` 枚举、[crates/auction-http/src/assets/routes.rs](crates/auction-http/src/assets/routes.rs) 与[docs/contracts/routes.md](docs/contracts/routes.md)。

## 验证范围

默认门禁只做格式、编译与静态分析，不运行测试，提交前必须通过：

```bash
just verify     # bash ci/checks/check.sh all fmt check dependency-versions lint wasm rustdoc
```

它覆盖四种编译配置：宿主默认 feature（`check`）、宿主全 feature（`lint`）、Public 与 Admin 两组 wasm32（`wasm`），外加 `rustdoc`。只跑单项时要注意：`cfg(target_arch = "wasm32")` 代码只会被 `wasm` 项检查。

行为变更必须随功能维护并运行受影响范围的测试；已有测试保留，修改行为时同步正反例与回归断言。

| 入口 | 范围 |
| --- | --- |
| `just test` | 纯逻辑、单元与本地替身测试；不要求数据库，不连接真实外部账号 |
| `just test-db` | 真实 PostgreSQL 18 上的锁、隔离级别、幂等与迁移语义；未设置 `AUCTION_TEST_DATABASE_URL` 时用本机 PG18 二进制启动临时集群 |
| `just test-db-compat` | 只验证迁移兼容声明 |
| `cargo test --locked -p auction-storage --features test-db --test postgres -- --ignored perf` | 数据库热点基准；改动热路径查询、索引或通知提交路径时运行 |

可按改动所属 owner 缩小范围（如 `cargo test --locked -p auction-domain --lib`），交付说明写出实际执行的命令与未覆盖范围；测试类别、环境要求与覆盖限制见[docs/development.md#测试](docs/development.md#测试)。默认测试不得要求数据库或连接真实外部账号，数据库测试只在显式入口运行。

浏览器、真实服务、手工冒烟等其他行为验证需要维护者明确要求，并先说明范围与环境；启动应用服务、访问健康接口或走功能流程不属于静态门禁。

## Git 与交付

- 修改前查看 `git status --short`，保留与任务无关的改动。未经授权不执行 `git reset --hard`、`git checkout --`、强制推送等破坏性操作。
- `AGENTS.md`、`.gitignore`、`.editorconfig`、`README.md`、`Cargo.lock` 必须纳入版本控制且不被忽略（`repository` 门禁）。`CLAUDE.md` 只引用本文件。
- 不提交凭据、带密码的 URL、私有 TOML、`.keyenv`、本地环境文件、`target/`、临时制品或机器专属绝对路径。
- 提交标题使用 `type(scope): summary`，如 `fix(storage): …`、`perf(web-public): …`，summary 用中文；正文说明保护的不变量与验证范围。按交付批次提交，每个批次是一个可独立审阅的提交。
- `main` 受保护，只经 PR 合并：要求线性历史、分支与 `main` 同步、必需检查 `docs`/`verify`/`security-audit`/`msrv` 通过，且全部讨论串已解决；`test` 作业不是必需检查，但同样必须通过。自动审查提出的问题逐条核实：成立的修复后回复提交号，不成立的回复理由，然后解决讨论串。
- 正式发布只在 `main` 上打 `v<workspace version>` tag 触发；部署只接受该 tag 产生的 GitHub Release 资产，本地构建或分支候选制品不可部署。

功能交付顺序：

1. 明确路由、JSON、事件、配置、数据库与部署契约。
2. 在领域定义规则，在应用层建立用例与端口；设计锁、幂等、审计与事件。
3. 接入存储/外部适配、HTTP、前端与 Worker；不复制状态机，不绕过 owner。
4. 同步契约、模块 README 与运维文档，执行 `just verify`。
5. 行为变更运行受影响范围的测试，记录实际执行与覆盖限制。

## 代理操作边界

- 推送或合并 `main`、打发布 tag 由维护者执行；代理的改动经功能分支与 PR 交付。
- 部署、生产写入与生产迁移只在维护者当次明确要求时执行，并按[ops/README.md#升级](ops/README.md#升级)走发布包内的 `auction-cli deploy`。其余时候生产环境只做只读排查：数据库查询放在只读事务中，不读取私有凭据文件，不解密诊断证据。
- 不连接真实外部账号（MoogleMail、论坛），不改动与任务无关的本机服务、数据库、端口与浏览器会话。
- 业务语义、破坏性操作或策略取舍需要决定时，先问维护者。

## 已知陷阱

- 批量改导入或模块名时，`cargo fix` 只看非测试构建，会删掉只被测试模块使用的父模块导入；完成后编译测试目标，并在测试模块补上显式导入。
- `just web-build` 在构建前后各计算一次输入指纹，构建期间修改输入文件会以“Web 构建期间输入发生变化”失败；构建时不要并行编辑仓库。
- 修改 `ci/release/build-identity.sh` 会让受控工具的配方指纹失效，需要重装 `dx` 与 `sccache`；构建后处理放在 `ci/release/web-bundle.sh`。
- 本机编译加速（sccache、mold 等）只配置在用户级 `~/.cargo/config.toml`，不改仓库 profile；发布脚本会清空 Cargo 配置中的 rustc 包装器，不影响发布构建身份。多个工作树并行构建时各用独立的 target 目录。
