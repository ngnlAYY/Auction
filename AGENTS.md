# AGENTS.md — Auction Rust

本文是 Auction Rust 仓库的项目级协作规范，供 AI 编程代理和人类贡献者共同遵守。
它说明当前目录结构、模块边界、代码风格、配置方式、验证范围和交付约束。最后一次按
工作区核对时间为 2026-09-04。

## 项目概览

Auction Rust 是一个自包含的全 Rust 论坛生态拍卖系统。业务行为、HTTP/事件契约、部署方式和
数据语义必须由本仓库内的源码、迁移和项目文档定义；实现、构建和验收不得依赖外部源码仓库。

- 产品是面向论坛生态的拍卖系统，包含公开竞价、管理端和负责生命周期、论坛及通知任务
  的后台 Worker。
- Public、Admin、Worker 是可以独立部署的进程边界；`auctionctl` 负责配置、迁移和运维
  命令行操作。
- 浏览器端同样使用 Rust：Dioxus 客户端渲染，产物编译为 `wasm32-unknown-unknown` 后由
  后端内嵌提供。不引入
  React、TypeScript、Node 或第二套前端构建链。
- “全 Rust”只约束本仓库代码和直接引入的程序库使用 Rust；PostgreSQL、SQLite、Redis、
  反向代理和操作系统服务等外部组件不受实现语言限制。
- PostgreSQL 是完整生产后端；SQLite 是面向本地、演示和单主机低并发部署的 Lite 替代
  后端。敏感 TOML 中的 `database_url` 选择档位，不能通过 `AnyPool` 抹平事务、锁、迁移
  和能力差异，也不要求 SQLite 达到 PostgreSQL 的系统能力。
- 拍卖页面的静态结构、CSS 和交互视觉语言由本仓库 Web crate 与样式资源共同定义；页面样式完成
  不等于业务功能或运行验收已经完成。
- 当前状态是**功能与运行证据收口阶段**：主要模块、HTTP/浏览器路径、PostgreSQL 完整档、SQLite
  Lite 声明子集、进程生命周期和运维边界已经建立。竞价参与者资格锁、黑名单后的代理冻结/会话撤销、
  慢速代理 Worker owner、论坛未知结果恢复、Public 实时 client/server v1 wire，以及代理模式、严格
  JSON/业务方法、错误外壳、Public 规则 locale 和登录返回已经静态收口；HTTP 边界已有隔离环境局部
  R1。canonical PostgreSQL v28 历史格式的隔离接管及灾备链也已取得 R1，API-only/embedded 制品和
  确定性 archive 已建立显式门禁；真实外部服务、非空生产数据、目标主机部署切换和完整浏览器逐页
  验收仍缺运行态证据，因此不得宣称生产验收完成。
- 除非用户明确要求，暂缓新增或运行单元测试、集成测试、功能测试用例、端到端测试和浏
  览器测试。已有测试文件保留，不因这条规则删除。
- 根目录 [`README.md`](README.md)、[`docs/architecture.md`](docs/architecture.md)、
  [`docs/tech-stack.md`](docs/tech-stack.md)、[`docs/contracts/`](docs/contracts/) 下的契约清单以及各目录的
  `README.md` 都属于项目契约。
  架构、外部行为或目录职责变化时要同步更新。

## 仓库结构

```text
.
├── Cargo.toml / Cargo.lock       # Rust 工作区与锁定后的依赖图
├── rust-toolchain.toml           # Rust 1.98.0 当前 stable、rustfmt、Clippy 和 WASM 目标
├── rustfmt.toml                  # Rust 格式化策略
├── clippy.toml                   # Clippy 与最低支持 Rust 版本策略
├── .editorconfig                 # 多语言编辑器默认设置
├── Justfile                      # 本地开发与默认静态验证命令
├── README.md                     # 项目概览与启动说明
├── AGENTS.md                     # 本文件，随源代码版本控制
├── apps/                         # 薄的装配根和可部署二进制
│   ├── public-api/               # 公开 HTTP 表面与公开端前端产物服务
│   ├── admin-api/                # 管理 HTTP 表面与管理端前端产物服务
│   ├── worker/                   # 持久化生命周期/集成 Worker 入口
│   └── auctionctl/               # 配置、迁移和运维命令行工具
├── crates/                       # 按模块所有权拆分的可复用能力
│   ├── auction-domain/           # 纯实体、值对象和领域不变量
│   ├── auction-application/      # 用例、端口和应用错误
│   ├── auction-contracts/        # 稳定的 HTTP/流 DTO 与错误契约
│   ├── auction-http/             # Axum 路由与 HTTP 表面适配器
│   ├── auction-web-shared/       # 浏览器端共享设计变量、基础组件与语言选择
│   ├── auction-web-public/       # 公开端 Dioxus 组件、路由与 i18n/*.ftl
│   ├── auction-web-admin/        # 管理端 Dioxus 组件、路由与 i18n/*.ftl
│   ├── auction-storage/          # PostgreSQL 完整与 SQLite Lite 的 SQLx 适配器边界
│   ├── auction-cache/            # 可禁用的缓存边界与 Redis 适配器
│   ├── auction-integrations/     # E-Hentai、MoogleMail 和外部网关
│   ├── auction-jobs/             # Worker 调度与任务编排
│   ├── auction-config/           # TOML 解析与进程配置
│   ├── auction-observability/    # tracing、metrics、健康检查和关联 ID
│   └── auction-process/          # 终止信号、有界 drain 和任务所有权原语
├── migrations/                   # PostgreSQL 完整 schema 与 SQLite Lite 子集迁移
├── docs/                         # 架构、技术选型和版本化契约
├── ops/                          # 配置示例与部署资料
├── ci/                           # 持续集成约定和后续辅助脚本
└── .github/workflows/            # 仓库静态验证工作流
```

目录形状参考 `daeuniverse/honk` 的工作区、`crates/*` 拆分和根目录工程约定，但本项目只
借鉴工程组织方式，不复制 Honk 的代理或 eBPF 业务模块。

### src 子目录规则

所有 apps/*/src 和 crates/*/src 都必须按职责使用清晰的子目录存放实现源码。
根目录只保留必要的装配入口：二进制保留薄的 main.rs，库保留薄的 lib.rs；
入口以外的实现文件不得平铺在 src/ 根目录。

- 子目录名称使用领域职责或适配器职责，例如 domain/、ports/、services/、
  storage/、http/、web/、bootstrap/，禁止使用 misc/、common/ 一类无法表达
  owner 的兜底目录。
- 每个职责目录必须有单一 owner；目录边界不能通过大而全的 mod.rs 或共享结构体绕开。
- 同一目录出现三个或更多同前缀实现文件时，默认视为缺少职责子目录；确有同层必要时必须先
  调整命名消除伪分组，或在项目规范中记录例外，不能绕过布局检查。
- 只有一个实现文件的模块也要放进有意义的职责目录；该目录可以使用 mod.rs 作为入口。
- crate 根的 lib.rs/main.rs 只负责模块声明、公开重导出、参数装配和进程生命周期；
  领域规则、SQL、HTTP 处理器、Dioxus 组件和外部调用必须放在对应子目录。
- 新增或移动源码时同步更新该 crate 的 README.md、架构 owner 和相关迁移阶段说明。

## 技术栈

- **语言：** Rust 1.98.0 当前 stable，edition 2024；workspace MSRV 为 1.94.1。Rust 官方
  没有 LTS，且只支持最新 stable，不能把 MSRV 当作 LTS。
- **异步运行时：** Tokio 1.51.x 官方 LTS，当前下限 1.51.4。
- **HTTP 服务：** Axum 0.8.9，使用其 Tower 中间件模型；连接层直接使用同依赖图的
  Hyper/`hyper-util` 配置请求头读取超时、HTTP/2 CONNECT 和连接排空。当前没有为尚未使用的
  能力直接引入 `tower-http`。
- **浏览器界面：** Dioxus 0.7.10 纯客户端渲染；产物由 `just web-build`（内部调用
  `dx bundle --platform web`）生成，
  编译目标为 `wasm32-unknown-unknown`，再由 `auction-http` 用 `rust-embed` 内嵌。
  不启用 `dioxus-fullstack`/`dioxus-server`，避免第二套 HTTP 服务器与 `auction-http`
  的路由所有权冲突。
- **国际化：** `dioxus-i18n` 0.5.1（Project Fluent 后端）与 `unic-langid` 0.9.6；
  译文以 `include_str!` 在编译期内嵌，首屏不额外请求语言资源。
- **数据库：** SQLx 0.9；PostgreSQL 与 SQLite 驱动由独立 Cargo feature 选择，发布制品
  只编译实际需要的驱动。PostgreSQL 18.x 提供完整生产能力，跟进 major 18 的当前修复版；
  SQLite 只提供 Lite 能力子集，当前锁图捆绑 SQLite 3.51.3。已支持的关键竞价事务使用
  显式 SQL、事务、后端对应的锁、唯一约束和追加式迁移。
- **缓存：** 默认禁用；启用 `cache-redis` Cargo feature 并在配置中选择 `redis` 后使用
  `redis` crate 1.6.0 对接 Redis。缓存只保存可重建数据，不能参与竞价正确性、幂等、锁、
  会话权威状态或审计。
- **HTTP 客户端：** reqwest 0.13.4，外部服务使用 rustls。
- **序列化：** serde 和明确的契约 DTO；当前没有为未使用的手工 JSON 路径直接引入
  `serde_json`。
- **错误处理：** 库、领域和应用层使用 `thiserror`；二进制装配根可以使用
  `anyhow::Context`。
- **可观测性：** tracing、tracing-subscriber、结构化字段和 JSON 输出。
- **命令行：** clap 4.6.6。
- **标识与时间：** 使用带类型的 UUID 包装和 UTC 时间戳。
- **运行配置：** 统一使用 TOML；三个角色配置引用同一份公共非敏感 TOML，并分别引用独立敏感 TOML。数据库连接
  串、Redis 连接串、Cookie 和会话签名密钥只允许出现在敏感文件中；MoogleMail 本地联调
  允许通过 `auction-config` 的严格 loader 读取 `.keyenv` 中固定的两个键，但这不是通用
  环境变量覆盖；环境变量 `AUCTION_CONFIG_FILE` 仅用于选择角色配置文件。

具体取舍见 [`docs/tech-stack.md`](docs/tech-stack.md)。只有上游正式提供的版本才能称为
LTS：Tokio 使用官方 LTS；Rust、Axum、Dioxus、SQLx 等没有官方 LTS 的组件使用受支持的
稳定版本，不得伪称 LTS。Rust 构建工具链跟随当前 stable，独立 CI job 校验 MSRV。依赖
精确解析版本以 `Cargo.lock` 为准，本地与 CI 的静态检查使用 `--locked`；升级时按组件
检查 Rust MSRV、浏览器端 WASM 构建兼容性和已实现数据库适配器。

## 依赖方向与模块所有权

依赖方向向内。装配根负责组合适配器，领域层不应该知道自己通过什么协议交付或持久化。

```text
apps/*
  ├── auction-http / auction-web-* / auction-jobs
  ├── auction-storage / auction-cache / auction-integrations
  ├── auction-config / auction-observability / auction-process
  └── auction-application
              ↓
       auction-domain
```

必须遵守以下边界：

- `auction-domain` 只能使用领域安全的依赖，例如 serde、UUID、时间类型和 `thiserror`。
  不得导入 Axum、Dioxus、SQLx、reqwest、环境变量读取或具体日志初始化。
- `auction-application` 只拥有用例、事务编排和窄端口。不得读取进程配置、打开网络监听、
  直接执行 SQL 或直接调用外部服务。
- `auction-contracts` 只定义稳定的输入输出、流消息和错误契约。不要把数据库记录直接当
  作浏览器 DTO。
- `auction-storage` 实现应用端口；SQL 模型、查询、连接池、事务和后端锁策略只放在这里，
  业务决定留在领域/应用层。
- `auction-cache` 只负责可丢弃、可重建数据的缓存适配；默认禁用，Redis 是可选实现。缓存
  miss、不可用或失效失败必须回退到权威数据源，不得改变命令结果。
- `auction-integrations` 实现外部传输和认证。外部响应必须经过应用命令或持久化 Worker
  操作，不能直接修改拍卖状态。
- `auction-http` 负责 HTTP 输入到应用命令、应用输出到稳定契约的映射。处理器不得包含
  竞价、结算或状态机规则。
- `auction-web-public` 与 `auction-web-admin` 各自负责自己 surface 的展示、路由、加载和
  用户交互，不得导入 SQLx、数据库模型、密钥或外部集成客户端，也不得互相依赖。
  `auction-web-shared` 只放真正通用、无业务偏向的基础能力。
- `auction-jobs` 负责调度、重试、游标和 Worker 编排，不得创建第二套拍卖状态机。
- `auction-config` 负责 TOML 解析和配置校验；不要在业务模块中散落环境变量读取。
- `auction-observability` 负责日志、指标、健康检查和关联信息；敏感值不得进入日志。
- `auction-process` 只提供终止信号、有界 drain 和 abort-on-drop 任务守卫；不得读取配置、打开
  监听器、发布心跳或决定业务任务的停止顺序。
- `apps/*` 是装配根，只负责参数/配置加载、依赖注入、监听器绑定、优雅关闭和退出码。
- Public 与 Admin 是不同表面。公开端不得依赖管理端 UI 模块或管理专属命令。
- 跨模块调用必须通过窄端口或明确契约，禁止访问另一个 crate 的私有持久化模块。

### 模块化工作方式

一个功能应按“领域规则 → 应用用例/端口 → 存储或外部适配器 → HTTP/Web/Worker 表面”
逐层落地，而不是把所有逻辑塞进路由处理器、单个巨型模块或二进制入口。

- 一个模块只负责一组内聚职责，并且只有一个明确所有者；表、事件、路由、配置字段和写
  入查询都不能出现多个隐含 owner。
- 新增功能前先确定它属于领域、应用、契约、适配器还是表面；跨层代码拆成窄接口，不用
  共享大结构体绕过边界。
- 保持模块和函数小而可组合；出现“上帝模块”、重复状态机或跨层条件分支时，先拆分责
  任再继续加功能。
- 依赖只向稳定的内层或端口流动。具体数据库、HTTP、外部服务和框架实现不能反向污染
  领域模型。
- 兼容既有持久化格式或已发布 wire 时建立防腐层或映射，不把历史表、DTO 或命名传播到新领域。
- Public、Admin 和 Worker 的差异显式建模；只有真正稳定且无业务偏向的基础能力才进入
  共享模块。

## 领域不变量

任何修改拍卖写入、状态迁移、结算或后台任务的变更，都必须在代码或变更说明中写明它保
护了哪些不变量。初始状态机位于
[`crates/auction-domain/src/auction/mod.rs`](crates/auction-domain/src/auction/mod.rs)，它只是
第一片领域切片，不能视为完整竞价行为。

以下不变量适用于 PostgreSQL 的完整能力，也适用于 SQLite Lite 已声明支持的对应能力。
Lite 可以不实现某项系统功能，但不能用“轻量版”为已实现写入降低正确性要求。

1. 每个可变命令都有稳定的 `command_id`。重复提交相同命令返回原结果；同一 ID 搭配不同
   负载必须报冲突。
2. 竞价事务使用所选数据库后端的锁模型保护相关拍卖、标的和有效授权，校验资格与状态，
   计算结果并原子写入。PostgreSQL 竞价和关键生命周期写入使用 Serializable、最多 3 次
   序列化冲突重试并配合明确行锁；队列领取等路径才可使用 Read Committed 加显式锁。
   SQLite 使用明确的串行写事务。禁止先读价格、再在另一个事务写入竞价。
3. 成功竞价必须在同一数据库事务中写入竞价事实、状态版本、审计事件和持久化事件/outbox
   记录。
4. 论坛发帖、邮件、通知和其他外部写入发生在本地事务提交后；外部失败不能抹掉已接受的
   竞价。
5. Worker 游标和命令账本必须持久化；内存定时器或内存游标不足以支持重放、重启和故障转
   移。
6. 非法状态迁移必须失败关闭；不得静默修正非法金额、未知标的编码、无效身份或过期版本。
7. 会话凭据、论坛 Cookie、邮件 Cookie 和其他密钥材料不得出现在日志、DTO、快照或提交的
   配置中。
8. 审计记录只允许追加；更正应记录为新的事实/事件，不能编辑历史证据。
9. 缓存不是权威状态。竞价金额、资格、幂等账本、锁、会话真值和审计证据必须由数据库或
   明确的权威服务决定；禁用或丢失缓存不能破坏正确性。

## 命名规则

命名优先表达领域含义，再遵循 Rust 约定。不要为了短而使用无法从上下文确定含义的缩写。

- Rust 模块名、源文件名、函数名、方法名、局部变量名和字段名使用 `snake_case`。
- `struct`、`enum`、`trait`、类型别名和 Dioxus 组件使用 `PascalCase`，例如
  `AuctionId`、`AuctionStatus`、`AuctionRepository`、`PublicAuctionCatalog`。
- 常量和全局静态值使用 `SCREAMING_SNAKE_CASE`，例如 `MAX_BID_AMOUNT`；普通配置字段不
  使用全大写。
- 类型中的缩写保持 Rust 风格并前后一致，使用 `HttpClient`、`ApiError`、`AuctionId`，
  不要在同一概念上混用 `HTTPClient`、`Httpclient`、`auctionID` 和 `auction_id`。
- 标识符必须带领域上下文：优先使用 `auction_id`、`lot_id`、`command_id`，不要用含义不
  明确的 `id`、`data`、`item` 代替关键业务字段。
- Dioxus 组件的 props 使用 `#[component]` 函数参数表达；组件事件和状态命名应表达用户
  动作或状态变化。
- URL 路径、事件名和 JSON 字段必须遵守本仓库已冻结的版本化契约；新增契约使用小写和连字符
  的 URL 路径、`snake_case` 的 JSON/TOML/SQL 字段，并通过 serde 映射历史兼容命名。
- SQL 表名、列名、索引名和迁移中的逻辑名称使用 `snake_case`；PostgreSQL 与 SQLite
  Lite 中都存在的领域对象保持同名和同一逻辑含义，不要求完整 schema 一一对应。
- CSS 类使用小写连字符和 BEM 结构，例如 `public-auction-catalog`、
  `public-auction-catalog__name`、`public-auction-catalog--compact`，禁止使用驼峰命名。

## 中文注释与文档要求

所有新增或修改的代码注释，默认使用中文。需要注释的内容包括模块职责、公共 API、领域
不变量、事务/锁/幂等策略、兼容旧契约的映射、外部服务限制以及不明显的安全或性能取舍。

- 模块级说明使用中文 `//!`，公共类型、函数和端口使用中文 `///`；复杂分支使用中文
  `//` 说明原因、约束或失败后果。
- 注释解释“为什么”、不变量、线协议和兼容决策，不要逐行复述显而易见的代码。能通过
  更清晰命名表达的内容不要用长注释掩盖。
- 协议名称、crate/API 标识符、SQL 关键字、外部系统固定名称和必须逐字匹配的契约字符串
  可以保留英文；围绕这些标识符的解释仍使用中文。
- 修改已有英文注释时，在本次触及范围内翻译成中文；不要为了改语言而无关地大面积重写
  代码。
- 所有新增或修改的 `README.md`、架构说明和运维说明使用中文。命令、路径、代码符号、
  产品名和外部链接不翻译，但正文说明必须保持中文。

## 代码风格

- Rust 文件使用仓库 `rustfmt.toml`；提交前运行 `cargo fmt --all -- --check`，不得手工
  维护与格式化器冲突的排版。
- Clippy 使用 `-D warnings`。不要通过关闭 lint 隐藏设计问题；确有必要时，在最小范围内
  用中文注释说明原因。
- 导入按 rustfmt 结果保持清晰；函数保持单一职责，优先使用早返回、明确的错误类型和
  可读的控制流。
- 库错误使用 `thiserror` 并保留领域错误分类；只有二进制装配根可以使用
  `anyhow::Context` 汇总启动错误。
- 使用结构化 `tracing` 字段，例如
  `info!(auction_id = %id, command_id = %command_id, "bid accepted")`。日志消息可保留
  稳定的英文事件名，但字段名必须清楚且禁止记录密钥、Cookie、密码和完整凭据。
- 请求、Worker 和持久化路径避免 `unwrap`/`expect`。只有经过说明的启动期不可恢复不变量
  才可以让进程失败，并且错误必须包含可行动的原因。
- 公共 DTO、数据库记录和领域聚合保持不同，即使当前字段看起来相同；转换应在边界完成。
- 配置键、JSON 字段和 SQL 字段默认使用 `snake_case`；不要把运行时设置散落在环境变量中。
- CSS 使用两空格缩进、`auction-web-shared/style/base.css` 中的设计变量和 BEM 风格特性类；
  surface 专属规则放在各自 crate 的 `assets/` 下。
- 只做与当前需求相关的最小修改；不要在迁移一个能力时顺手进行无关重构或改名。

## 配置规则

- 所有运行时配置文件必须是 TOML，公共/角色/敏感配置示例位于 `ops/config/examples/`，Admin runtime
  配置示例集中在 `ops/config/examples/runtime/`。
- `AUCTION_CONFIG_FILE` 只能选择配置文件；单个配置项不能通过环境变量覆盖，业务模块也
  不能自行读取环境变量。
- Public、Admin 与 Worker 角色配置必须通过必填 `common_file` 引用同一份公共非敏感 TOML；公共
  文件拥有日志过滤器、指标、缓存和拍卖策略，角色文件拥有 listener、tracing、realtime、会话或
  Worker 调度，并通过必填 `secrets_file` 引用敏感 TOML。相对路径以角色配置文件所在目录解析。
- Admin/Worker 可以共享 `runtime_config_file`。其 `operations` 五组分别接管后续新拍卖经济模板、
  自动排期、论坛同步、告警和 Worker 循环参数；每组 `managed` 默认关闭，沿用公共/角色配置。
  保存后重启对应进程才生效，不回写已有拍卖快照、任务游标或租约。接管任一组时部署预检必须
  核对两者引用同一 runtime 文件及有效拍卖模板/排期一致；MoogleMail transport、凭据、邮件调度、
  mailbox identity 和全局启用仍由部署配置拥有。
- `database_url`、`redis_url`、论坛 Cookie、`session_signing_key`、`admin_actor` 和
  `admin_secret` 只能放在敏感 TOML；MoogleMail Cookie 生产路径同样使用敏感 TOML，但本地
  联调可由 `auction-config::load_mooglemail_keyenv` 严格读取 `.keyenv` 的
  `hv-ck-key1`/`hv-ck-key2`。Worker 只使用 `hv-ck-key1`；`hv-ck-key2` 只允许显式隔离验证，
  不得加入运行时候选或自动切换。论坛和游戏邮件每种凭据全局最多一条 active Cookie。这些字段
  不通过通用环境变量覆盖；Admin 凭据必须成对配置，secret 至少 16 字节。
- 敏感文件不得提交，部署时应限制为仅运行账号可读。配置错误和日志不得回显连接串、
  Cookie、密钥或包含这些内容的 TOML 原文。
- 敏感文件中的 `database_url` 支持 `postgres://`、`postgresql://` 和 `sqlite:`，并直接
  决定部署档位。
  PostgreSQL 是完整生产后端；SQLite 是本地、演示和明确单主机低并发部署的 Lite 后端，
  每个角色至多一个实例、Worker 并发为一，并说明串行写入、锁等待、备份和本地文件系统
  假设。不得把 SQLite 文件放在网络共享文件系统上作为横向扩展方案。
- 配置字段变化必须同时更新解析代码、示例、相关 README 和部署说明；当前新配置格式不
  兼容旧的单文件敏感字段，旧格式必须以明确错误失败。
- 受版本控制的 TOML 示例中，每个赋值字段前必须紧邻中文注释并明确标为“可选”或“必填”；
  同一条概括注释不能替代各字段自己的状态、默认值或启用条件说明。

## 数据库与迁移

- PostgreSQL 是完整生产目标；SQLite 是非对等 Lite 目标。稳定领域语义可以共享，但
  SQLite 只实现 Lite 能力需要的应用端口；连接池、SQL、事务、锁、能力缺失和迁移必须在
  `auction-storage` 与对应文档中显式表达。
- PostgreSQL 部署固定 major 18 并及时升级 18.x 修复版本；major 升级必须单独评审迁移与
  回滚。SQLite 引擎版本由 `Cargo.lock` 中的 SQLx/`libsqlite3-sys` 锁定，依赖升级时必须
  核对实际捆绑版本和编译选项。
- 新增持久化能力前必须在 [`docs/architecture.md`](docs/architecture.md) 标明它属于
  “完整档”“Lite 档”或“两者”；只有标为 Lite 或两者的能力才要求 SQLite 实现。
- `migrations/postgres/` 维护完整生产 schema，`migrations/sqlite/` 维护 Lite 所需子集。
  两个迁移树不要求文件、序号或数据库对象一一对应；共有对象必须同名同义。迁移只允许
  追加，已发布文件禁止修改。
- 新结构必须先对照领域所有权和 [`docs/contracts/schema.md`](docs/contracts/schema.md) 评审，不能
  未经审计复制历史表。需要兼容读时使用防腐层或明确的兼容适配器。
- 每个表、迁移、写入查询、事件和路由只有一个 owner；新增对象时在架构说明中记录职责。
- SQLx 查询宏需要其所属后端具备可复现的 schema/check 路径。采用离线查询校验后，只更新
  受影响的迁移树和查询元数据；共有查询才要求两个后端都校验。
- 破坏性数据库操作必须有明确运行手册、精确目标确认和可验证备份；不要在开发脚本中随
  意加入 `DROP DATABASE` 一类命令。

## Web 规则

- 所有 Public/Admin 业务表格使用 100% 容器宽度，每张表按字段的预计内容、时间格式和操作数量
  独立定义完整百分比列宽，比例由业务组件拥有；Markdown 表格按表头与典型内容长度计算权重。
  具体分配见 [`docs/table-layouts.md`](docs/table-layouts.md)。禁止通过固定最小宽度或横向滚动条
  展示宽表；窄屏保持表头与行结构，超长内容直接截断并
  提供完整文本悬浮提示，复杂文本同时支持键盘或触控查看。不得重新引入窄屏表格卡片化。
- Public 与 Admin 的组件树、路由树和静态产物保持分离，分别由 `auction-web-public` 与
  `auction-web-admin` 拥有；只有真正通用且没有业务偏向的基础组件才进入
  `auction-web-shared`。公开端二进制不得包含管理端资源。
- 浏览器端是纯客户端渲染的 Dioxus SPA。不启用 SSR 或 hydration：它们不替换当前产品能力，只会新增
  hydration mismatch 和双目标构建。
- 核心竞价、认证和管理命令必须保留显式、版本化的 REST/JSON 契约。Public 实时更新使用
  带 sequence/version 的 WebSocket 通知；跳号或重连后读取权威快照。无法保持长连接的客户端可
  退化为 SSE。
- 若要切换到 Askama + HTMX 一类服务端渲染方案，必须先由用户明确放宽“浏览器端全 Rust、
  禁止 JavaScript”契约，不能把技术原型直接变成架构变更。
- 前端静态产物由 `just web-build`（内部调用
  `dx bundle --platform web --profile wasm-release`）生成并复制到稳定目录，
  再通过 `rust-embed` 内嵌进后端二进制。
  内嵌声明必须保留 `allow_missing`：未构建前端时后端仍要能编译，运行时以明确的 503
  报告，不得 panic。
- Public/Admin 只有显式 `web-embed` feature 才能选择各自稳定资源目录；API-only 使用固定空资源
  集合，不能根据构建机上是否残留 `target/dx` 隐式改变制品。
- 界面文案一律经由 `dioxus-i18n` 从各 surface 的 `i18n/*.ftl` 取用，**不得在组件里硬编码
  用户可见字符串**。受支持语言为 `en`、`zh-CN`、`zh-TW`、`ja`、`ko`；公开端默认 `en`，
  管理端默认 `zh-CN`。两个 surface 的译文资源彼此
  隔离，共享的只有 `auction-web-shared` 提供的语言集合、持久化和 `<html lang>` 同步能力。
- 语言偏好存储键是已发布的浏览器契约，固定为 `hv.lang`，不得重命名；
  存储值必须在受支持集合内，否则回退到该 surface 的默认语言。
- Fluent 消息 ID 不允许包含点号，使用 `nav-home` 而不是 `nav.home`。新增文案时
  必须同步更新全部 5 个 `*.ftl`——`t!` 在键缺失时 panic，缺一种语言就会让该语言下的页面崩溃。
- API/流 DTO 放在 `auction-contracts`，浏览器不能直接接收 SQL 行或数据库内部模型。
- 会改变竞价或管理状态的用户动作必须携带 `command_id`，并显式展示冲突与幂等结果。
- 拍卖页面的 CSS 和静态资源必须遵守本仓库已冻结的结构与视觉契约，业务逻辑必须留在 Rust；
  不得引入 React、TypeScript、npm 包依赖或第二套前端构建系统。dx CLI 会
  按需下载 esbuild 作为构建期工具，这是唯一允许的例外，且不得成为运行时依赖。
- 当前 Public/Admin 页面已接入版本化 API；不得重新加入会被误认成权威状态的示例竞价行或管理
  数据。演示空态必须使用明确文案，并与契约、应用用例和持久化边界保持分离。

## 默认验证与测试范围

默认验证只覆盖格式、编译和静态分析，不覆盖行为测试：

```bash
# 查看可用任务
just

# 默认静态验证
just fmt
just repository-policy-check
just layout-check
just config-example-check
just i18n-check
just check
just lint
just verify

# 等价的直接命令
cargo fmt --all -- --check
just repository-policy-check
just layout-check
just config-example-check
just i18n-check
cargo check --locked --workspace --all-targets
cargo check --locked -p auction-storage --all-targets --no-default-features --features postgres
cargo check --locked -p auction-storage --all-targets --no-default-features --features sqlite
cargo check --locked -p auction-cache --all-targets --no-default-features --features redis
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings

# 校验 Rust 浏览器目标是否能够编译
cargo check --locked -p auction-web-public --target wasm32-unknown-unknown
cargo check --locked -p auction-web-admin --target wasm32-unknown-unknown
```

`cargo check --locked --workspace --all-targets` 只编译现有目标，不执行测试。除非用户特别
要求，不得编写或运行以下内容：

- 单元测试；
- 集成测试；
- 功能测试用例及其运行脚本；
- 端到端测试、浏览器测试和手工功能冒烟流程。

已有测试代码不删除、不扩展、不为了“顺手验证”而执行。若用户明确要求恢复测试，先说
明测试边界、SQLite/PostgreSQL 的差异和运行成本，再按要求实施；SQLite 只验证 Lite
能力契约，不能替代 PostgreSQL 的完整契约、隔离和行锁语义。

## 运行进程

公开端和管理端使用不同监听地址，默认分别为 `127.0.0.1:8090` 和 `127.0.0.1:8091`。
运行时配置通过 TOML 选择：

```bash
AUCTION_CONFIG_FILE=ops/config/examples/public.toml \
  cargo run -p auction-public-api

AUCTION_CONFIG_FILE=ops/config/examples/admin.toml \
  cargo run -p auction-admin-api

AUCTION_CONFIG_FILE=ops/config/examples/worker.toml \
  cargo run -p auction-worker

AUCTION_CONFIG_FILE=ops/config/examples/public.toml \
  cargo run -p auctionctl -- check-config public
```

前端产物构建需要 dx CLI（`cargo install dioxus-cli --locked`），命令为
`just web-build`；dx 会按需从 npm 注册表下载 esbuild 作为构建期工具。启动服务、
访问健康接口或执行功能流程属于行为验证，除非用户明确要求，本规范不要求运行。

## 变更与 Git 纪律

- 修改前先查看 `git status --short`，保留用户已有的无关改动。
- `AGENTS.md`、`.gitignore`、`.editorconfig` 和所有 README 都应纳入版本控制，不得在仓库
  `.gitignore` 中忽略这些项目契约文件；`CLAUDE.md` 只作为本地工具文件被忽略，不是项目契约。
- 应用工作区跟踪 `Cargo.lock`。`target/`、本地环境文件、私有 TOML 和临时发布产物被忽略。
- 禁止提交凭据、Cookie、含密码的数据库 URL、生成的 `target/` 内容和机器专属绝对路径。
- 变更必须限制在用户请求范围内。未经明确授权，不得使用 `git reset --hard`、
  `git checkout --` 等破坏性 Git 命令。
- 修改路由、状态码、JSON 字段、事件名、TOML 字段、配置选择器、迁移或部署产物时，必须
  同步更新对应契约和文档。
- 提交标题可使用 `type(scope): summary` 的简洁格式，例如
  `feat(domain): add auction state transition` 或 `docs(architecture): define worker ownership`。

## 交付工作流

功能按以下层次推进：

1. 在本仓库中登记并冻结路由、JSON、事件、配置、数据库和部署契约。
2. 在不依赖 HTTP、数据库和具体外部服务的情况下明确领域规则与状态机。
3. 先建立 PostgreSQL 完整结构、仓储、锁、幂等、审计和 outbox，再按 Lite 能力清单建立
   SQLite 子集；不要求迁移树或系统能力对等。
4. 为用户、规则、交付、实时通信和会话接入应用端口。
5. 按已冻结契约实现 Public 与 Admin 的 HTTP 表面。
6. 实现带持久化游标和重试策略的 Worker 任务与外部网关。
7. 完成 Dioxus 公开端/管理端页面，并保持本仓库视觉契约一致性。
8. 在用户明确要求行为验证后，再执行隔离环境、真实外部服务或切换前验证。

不要因为启动骨架可以编译就宣称功能或生产验收完成。每个交付阶段至少要有清晰的模块 owner、外部
契约和文档；测试相关的编写与运行只有在用户明确要求后才加入验收范围。

<!-- gitnexus:start -->
# GitNexus — Code Intelligence

This project is indexed by GitNexus as **Auction** (17604 symbols, 44683 relationships, 853 execution flows).

> Index stale? Run `node .gitnexus/run.cjs analyze --index-only` from the project root — it auto-selects an available runner. No `.gitnexus/run.cjs` yet? Bootstrap with `npx`, `bunx`, or `pnpm dlx` — e.g. `bunx gitnexus@latest analyze` (npm 11 npx crash; #1939).

## Always Do

- **MUST run impact analysis before editing.** Use `impact({target: "symbolName", direction: "upstream"})` (MCP) or `node .gitnexus/run.cjs impact "symbolName" --direction upstream --repo .` (CLI fallback); report callers, processes, and risk. Never substitute grep for graph analysis.
- **MUST analyze graph changes before committing.** Use `detect_changes({scope: "all"})` (MCP) or `node .gitnexus/run.cjs detect-changes --scope all --repo .` (CLI fallback). `partial: true` or `truncated: true` is not a clean check — a zero means unseen, not unaffected; re-run it. For regression review: `detect_changes({scope: "compare", base_ref: "main"})` or `node .gitnexus/run.cjs detect-changes --scope compare --base-ref "main" --repo .`.
- **MUST warn the user** if impact analysis returns HIGH or CRITICAL risk before proceeding with edits.
- **MUST treat `risk: UNKNOWN` as unresolved, not as low.** An empty caller set is not evidence the symbol is unused — it can also mean the callers are not resolvable by the index (plain-object property access, dynamic dispatch, cross-language calls). `impact` pairs `UNKNOWN` with a `riskNote` saying so. Confirm with a text search before treating the symbol as safe to change or delete; do not proceed on the strength of a zero.
- When exploring unfamiliar code, use `query({search_query: "concept"})` to find execution flows instead of grepping. It returns process-grouped results ranked by relevance.
- When you need full context on a specific symbol — callers, callees, which execution flows it participates in — use `context({name: "symbolName"})`.
- For security review, `explain({target: "fileOrSymbol"})` lists taint findings (source→sink flows; needs `analyze --pdg`).

## Never Do

- NEVER edit a function, class, or method before MCP/CLI impact analysis.
- NEVER ignore HIGH or CRITICAL risk warnings from impact analysis, and never read `UNKNOWN` as an all-clear — it means the walk could not answer, which is the one verdict that requires confirming by other means.
- NEVER rename symbols with find-and-replace — use `rename` which understands the call graph.
- NEVER commit before MCP/CLI graph change analysis.

## Resources

| Resource | Use for |
| --- | --- |
| `gitnexus://repo/Auction/context` | Codebase overview, check index freshness |
| `gitnexus://repo/Auction/clusters` | All functional areas |
| `gitnexus://repo/Auction/processes` | All execution flows |
| `gitnexus://repo/Auction/process/{name}` | Step-by-step execution trace |

## CLI

| Task | Read this skill file |
| --- | --- |
| Understand architecture / "How does X work?" | `.claude/skills/gitnexus-exploring/SKILL.md` |
| Blast radius / "What breaks if I change X?" | `.claude/skills/gitnexus-impact-analysis/SKILL.md` |
| Trace bugs / "Why is X failing?" | `.claude/skills/gitnexus-debugging/SKILL.md` |
| Rename / extract / split / refactor | `.claude/skills/gitnexus-refactoring/SKILL.md` |
| Tools, resources, schema reference | `.claude/skills/gitnexus-guide/SKILL.md` |
| Index, status, clean, wiki CLI commands | `.claude/skills/gitnexus-cli/SKILL.md` |

<!-- gitnexus:end -->
