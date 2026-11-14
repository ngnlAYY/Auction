# AGENTS.md — HV Auction 协作规范

本文件是项目级协作规范，适用于所有贡献者与自动化代理。业务、接口、数据与部署语义由本仓库独立定义，实现、构建与验收不依赖外部源码仓库。

[docs/README.md](docs/README.md) · [docs/architecture.md](docs/architecture.md) · [docs/development.md](docs/development.md) · [ops/README.md](ops/README.md)

## 项目概览

- HentaiVerse 装备拍卖系统：Public 竞价站点、Admin 管理后台，以及处理生命周期、结算履约、MoogleMail、论坛与告警的 Worker。
- Public、Admin、Worker 独立部署；`auction-cli` 负责配置检查、部署编排、迁移、备份恢复与业务运维；可选的 `auction-server` 在单主机上监管三个角色子进程。
- 原生程序仅支持 Linux x86_64（装配根在其他平台编译期报错）；浏览器包编译到 `wasm32-unknown-unknown`。通用制品不使用 `target-cpu=native`。
- 应用代码为 Rust；CI 与发布工具使用 Shell 等脚本。浏览器是 Dioxus CSR，WASM 由后端内嵌。PostgreSQL 18 是唯一数据库后端与唯一事实来源；不存在独立缓存进程。
- 普通启动从不隐式迁移。迁移只经 `auction-cli db migrate`、`auction-cli service start/restart`（停机门禁后）与合并部署首次初始化（网页明确确认后的专用空库）执行。

## 仓库结构

| 位置 | 职责 |
| --- | --- |
| `Cargo.toml` / `Cargo.lock` / `rust-toolchain.toml` | 工作区、锁定依赖、固定工具链与 WASM 目标、MSRV、profile、lint |
| `rustfmt.toml` / `.editorconfig` | 格式与编辑规范 |
| `Justfile` / `ci/` / `.github/workflows/` | 命令入口、静态门禁、发布脚本 |
| `apps/` | 五个可部署程序（薄装配根） |
| `crates/` | 按职责拆分的领域、应用、契约、适配器与前端 crate |
| `migrations/postgres/` | 只追加的迁移 |
| `docs/` / `ops/` | 设计与契约文档；配置示例与运维手册 |

### src 目录规则（`ci/checks/check.sh layout` 强制）

- `apps/*/src`、`crates/*/src` 根目录只保留薄 `main.rs`/`lib.rs`：模块声明、重导出、参数装配与进程生命周期，不放业务规则、SQL、HTTP 处理器、组件或外部调用。
- 实现放入有单一职责的子目录（即使只有一个文件）；使用 `ports/`、`services/`、`database/` 这类明确名称，不用 `misc/`、`common/` 兜底，不写巨型 `mod.rs`。
- 有子模块的模块以 `name/mod.rs` 为入口，叶子模块才是单个 `name.rs`；`name.rs` 与 `name/` 不得并存。
- 同一目录出现 3 个及以上同前缀的源码文件，说明缺少子目录，应拆分。
- 生产源码单文件不超过 600 行（文件末尾的 `#[cfg(test)]` 模块不计）；超过时按职责拆成子模块。
- 生产代码只用显式导入：不写私有 glob（`use super::*`、`use mapping::*`、`use self::别名::*` 等），父模块也不为子模块囤积导入；只放行外部 `prelude::*`、函数内枚举变体 glob 与 `pub use x::*` 门面再导出，测试模块可用 `use super::*`。每个 `use` 单独成句，不写 `use { … };` 总括分组。
- 文件与目录名使用 snake_case。

## 依赖方向与模块职责

装配根注入适配器；应用层只依赖窄端口与领域；依赖方向向内。完整说明见[docs/architecture.md#分层](docs/architecture.md#分层)。

| 模块 | 边界 |
| --- | --- |
| `auction-domain` | 纯领域规则；不导入 axum、dioxus、sqlx、reqwest，不读环境、不初始化日志、不访问外部服务 |
| `auction-application` | 用例、事务编排、应用错误与窄端口；不读进程配置、不监听、不直接执行 SQL 或外部调用 |
| `auction-contracts` | 前后端共用的稳定 DTO；数据库记录不能直接成为浏览器 DTO |
| `auction-storage` | 实现应用端口，拥有全部 SQL、连接池、事务与锁；业务决定留在内层 |
| `auction-integrations` | 外部传输与认证；响应经应用命令或持久化 Worker 处理，不直接改拍卖状态 |
| `auction-http` | 输入/命令与输出/DTO 映射；不包含竞价、结算或状态机规则 |
| `auction-jobs` | 调度、重试、租约与 Worker 编排，不建立第二套状态机 |
| `auction-config` | TOML 解析与校验；业务模块不得散落环境变量读取 |
| `auction-setup` | 首次设置、一次性令牌与初始化门禁；正式 Admin 不挂载设置写接口 |
| `auction-observability` | 日志、指标、tracing、心跳；敏感值不得进入日志 |
| `auction-process` | 信号、有界排空、任务守卫、本机 readiness 探针；不读配置、不监听 |
| `auction-web-public` / `auction-web-admin` | 各自的页面、路由、交互与译文；互不依赖，不导入任何后端 crate |
| `auction-web-shared` | 两端通用、无业务偏向的浏览器能力与设计 token |
| `apps/*` | 参数与配置加载、依赖注入、监听绑定、优雅关闭与退出码 |

功能按“领域 → 用例/端口 → 存储/外部适配 → HTTP/Web/Worker”顺序落地。每个表、迁移、查询、事件、路由与配置字段只有一个 owner；历史格式通过映射层兼容，不把旧命名传播到新领域。

跨装配根的共享机制同样只有一个 owner，`apps/*` 只做配置解析、绑定与生命周期，不复制第二份实现：HTTP listener/连接服务与三角色附加指标出口在 `auction-http`（`HttpListener`、`serve_http`、`spawn_metrics_listener`），tracing 收口在 `auction-observability`（`shutdown_tracing`），本机 readiness 探针在 `auction-process`（`probe_ready`），备份/部署共用的私有文件与子进程边界在 `apps/auction-cli/src/support`，浏览器 wire 谓词、`localStorage` 与一次性定时等待在 `auction-web-shared`（`wire`、`storage`、`timer`）。归属表与调用顺序见[docs/architecture.md#共享-helper-归属](docs/architecture.md#共享-helper-归属)。

## 领域不变量

修改写入、状态迁移、结算或后台任务时，必须在代码注释或变更说明中写明所保护的不变量。

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

## 命名

- 模块、文件、函数、变量、字段用 `snake_case`；类型、trait 与 Dioxus 组件用 `PascalCase`；常量用 `SCREAMING_SNAKE_CASE`；缩写按 Rust 风格（`HttpClient`、`ApiError`）。
- 业务标识带上下文：`auction_id`、`lot_id`、`command_id`，避免含混的 `id`/`data`/`item`。
- 新 URL 用小写连字符；JSON、TOML、SQL 名用 `snake_case`。已发布的路径、事件名与字段按契约保留。
- CSS 使用小写连字符与 BEM（如 `public-auction-catalog__name`），选择器带 surface 前缀（`public-`、`admin-`）。

## 注释与文档

- 注释与文档使用中文；协议、API、SQL 关键字与必须匹配的字符串保留原文。
- 每个非测试源码文件以 `//!` 说明模块职责（`ci/checks/check.sh module-docs` 强制），公共类型、函数与端口用 `///`；复杂分支用 `//` 解释原因、不变量、锁/幂等、兼容、安全或性能取舍。不写复述代码的注释，删除过期注释。
- Markdown 链接目标按当前文档所在目录计算相对路径，必要时使用 `../`，确保 GitHub 与本地渲染可直接点击；同页锚点使用 `#标题锚点`。正文与链接显示文字中的仓库文件路径仍以仓库根为起点（如 `docs/contracts/routes.md`、`crates/auction-http/src/assets/routes.rs`）。每项事实只在一个 owner 文档中详细描述，其他地方摘要并链接。
- 路由、状态码、字段、事件、配置、迁移或制品变化时，同步对应的 README 与契约文档（[docs/contracts/routes.md](docs/contracts/routes.md)、[docs/contracts/realtime.md](docs/contracts/realtime.md)、[docs/contracts/database.md](docs/contracts/database.md)、[ops/config/README.md](ops/config/README.md)）。`ci/checks/check.sh markdown-links` 离线检查相对链接与锚点，移动或重命名时须同步引用。

## 代码风格

- 使用仓库 rustfmt；Clippy 以 `-D warnings` 运行，不用关闭 lint 掩盖设计问题，必要的例外限制在最小范围并用中文说明。
- 函数单一职责，优先早返回与明确错误；库用 `thiserror`，只有装配根使用 `anyhow`。
- 请求、Worker 与持久化路径避免 `unwrap`/`expect`；不可恢复的启动不变量要给出可行动的错误。
- 结构化 tracing 字段；禁止记录完整凭据、密码、Cookie 或密钥。
- DTO、数据库记录与领域聚合分离，在边界映射。
- CSS 两空格缩进，颜色、字号、间距使用共享 [crates/auction-web-shared/style/base.css](crates/auction-web-shared/style/base.css) 变量；surface 专属规则留在各自的 `crates/auction-web-public/assets/` 与 `crates/auction-web-admin/assets/`。
- 只做需求相关的最小修改，不顺手重构或改名。新增 `pub` 接口前确认有调用方。

## 配置规则

权威说明见[ops/config/README.md](ops/config/README.md)。

- 运行配置一律 TOML；`AUCTION_CONFIG_FILE` 只选择角色文件，不覆盖任何字段；不引入通用环境变量覆盖。
- 三角色必须引用同一 `common_file`；`runtime_config_file` 要么三者引用同一个，要么都省略。
- 数据库、Cookie、Admin 凭据、数据密钥只进敏感 TOML；敏感文件不提交，只允许运行账号读取。错误信息不得回显 TOML 原文或秘密值。
- `ops/config/examples/` 中每个赋值字段前必须紧邻含“可选”或“必填”的中文注释，说明默认值与启用条件。示例同时是首次设置的生成模板。
- 每种外部 Cookie（邮件、论坛）全库同时最多一个 active。

## 数据库与迁移

- PostgreSQL 固定 major 18；驱动随 `auction-storage` 无条件编译，不设后端 feature，不使用 `AnyPool`。未配置数据库时进程以运行期骨架模式启动，不存在“无数据库”的编译形态。
- 迁移只追加，已发布文件禁止修改或删除；规则见[migrations/postgres/README.md](migrations/postgres/README.md)。新表同步[ops/deploy/retention-policy.v2.json](ops/deploy/retention-policy.v2.json)与[docs/contracts/database.md](docs/contracts/database.md)。
- 破坏性数据库操作只能经 `auction-cli` 的显式确认、可验证备份与回执完成；开发脚本不得加入 `DROP DATABASE`。

## Web 规则

- Public/Admin 是独立的 Dioxus CSR SPA，各自拥有路由、译文与资源；不启用 SSR、hydration 或 fullstack，不引入 JavaScript 框架、TypeScript 或 npm 运行时依赖（`dx` 构建时使用的 esbuild 是唯一例外）。
- `just web-build` 用 `dx bundle` 以 `wasm-release` profile 构建，复制到稳定目录后由后端 `web-embed` 内嵌；未构建时后端仍可编译，页面路由返回 503。只有显式 `web-embed` 才内嵌资源，Public 二进制不得包含 Admin 资源，反之亦然。
- 浏览器只消费 `auction-contracts` DTO；改变状态的动作携带 `command_id`，结果未知时只允许原样重试。不在浏览器重建业务状态机。
- 所有界面文案走各自 `i18n/*.ftl`：五种语言（en、zh-CN、zh-TW、ja、ko）键集合一致，键名用连字符、不含点号；Public 默认 en，Admin 默认 zh-CN。偏好键 `hv.lang`、`hv.theme` 是固定契约。
- 新增或删除页面时同步前端 `Route` 枚举、`crates/auction-http/src/assets/routes.rs` 与[docs/contracts/routes.md](docs/contracts/routes.md)。

## 验证范围

默认门禁保持格式、编译与静态分析，不运行测试：

```bash
just verify     # bash ci/checks/check.sh all fmt check dependency-versions lint wasm
```

行为变更必须随功能维护并运行受影响范围的测试：`just test` 覆盖纯逻辑、单元与本地替身，数据库语义使用 `just test-db` 和隔离的真实 PostgreSQL 18。测试类别、入口、环境要求与覆盖限制见[docs/development.md#测试](docs/development.md#测试)。默认测试不得要求数据库或连接真实外部账号；数据库测试必须隔离到显式入口。已有测试保留；修改行为时同步正反例与回归断言。其他行为验证（如浏览器、真实服务或手工冒烟）仍需用户明确要求，并先说明范围与环境；启动应用服务、访问健康接口或走功能流程不属于静态门禁。

## Git 纪律

- 修改前查看 `git status --short`，保留与任务无关的改动；未经授权不执行 `git reset --hard`、`git checkout --` 等破坏性操作。
- `AGENTS.md`、`.gitignore`、`.editorconfig`、`README.md`、`Cargo.lock` 必须纳入版本控制且不被忽略（`ci/checks/check.sh repository`）。`CLAUDE.md` 只引用本文件。
- 不提交凭据、带密码的 URL、私有 TOML、`.keyenv`、本地环境文件、`target/`、临时制品或机器专属绝对路径。
- 提交标题使用 `type(scope): summary`，正文说明保护的不变量与验证范围。

## 交付流程

1. 先明确路由、JSON、事件、配置、数据库与部署契约。
2. 在领域定义规则，在应用层建立用例与端口；设计锁、幂等、审计与事件。
3. 接入存储/外部适配、HTTP、前端与 Worker；不复制状态机，不绕过 owner。
4. 同步契约、模块 README 与运维文档，执行适用的静态门禁。
5. 行为变更运行受影响范围的 `just test` / `just test-db`，记录实际执行与覆盖限制；其他隔离环境、真实外部服务或上线前的行为验证按授权范围执行。
