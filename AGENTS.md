# AGENTS.md — Auction Rust

本文件是项目级协作规范。业务、HTTP/事件、数据及部署语义由本仓库独立定义；实现、构建和验收不得依赖外部源码仓库。

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [默认验证](docs/guide/development.md) · [文档维护](docs/guide/development.md#文档维护)

## 本页导航

[模块边界](#依赖方向与模块所有权) · [不变量](#领域不变量) · [配置](#配置规则) ·
[数据库](#数据库与迁移) · [Web](#web-规则) · [验证范围](#默认验证与测试范围) · [交付](#交付工作流)

## 项目概览

- Auction Rust 面向论坛生态，包含 Public 竞价、Admin 管理，以及生命周期、论坛和通知 Worker。
- 当前原生程序仅支持 Linux x86_64，五个装配根在编译期拒绝其他 OS/架构；正式发布使用 GNU/glibc。
  浏览器 `wasm32-unknown-unknown` 保留；通用制品不得默认使用构建机 `target-cpu=native`。
- Public、Admin、Worker 独立部署；`auctionctl` 负责配置、迁移和运维。可选 `auction-server`
  自管三角色子进程，仅支持 Linux/PostgreSQL 单主机；`auctionctl` 仍独立，默认分体发布不变。
- 角色及合并服务普通启动不隐式迁移。例外是网页明确确认后的专用空库初始化，以及分体
  `auctionctl deployment start/restart` 通过严格停机门禁后的迁移，详见[部署手册](ops/README.md)。
- 仓库代码及直接程序库使用 Rust；浏览器是 Dioxus CSR，WASM 由后端内嵌。PostgreSQL、
  Redis、反向代理及系统服务等外部组件不受语言约束。
- PostgreSQL 是唯一数据库后端。
- [实现与验证状态](docs/status.md)区分源码能力、历史验证和待补证据；页面完成、路由存在或静态编译
  不能证明生产验收。除非用户明确要求，不新增或运行行为测试，已有测试保留。
- 根 README、架构、技术栈、契约和各目录 README 均为项目契约；相关变化必须同步。

## 仓库结构

| 位置 | 职责 |
| --- | --- |
| `Cargo.toml` / `Cargo.lock` / `rust-toolchain.toml` | 工作区、锁定依赖、固定工具链与 WASM 目标 |
| `rustfmt.toml` / `.editorconfig` | 格式与编辑规范；Clippy 的 MSRV 继承 Cargo 包元数据 |
| `Justfile` / `ci/` / `.github/workflows/` | 开发任务、静态门禁与发布脚本 |
| `apps/{auction-public-api,auction-admin-api,auction-worker,auction-server,auctionctl}` | 薄装配根和可部署程序 |
| `crates/` | 按 owner 拆分的领域、应用、契约与适配器，见[架构](docs/design/architecture.md#crate-所有权) |
| `migrations/postgres` | PostgreSQL 追加式迁移 |
| `docs/` / `ops/` | 设计与契约、配置示例及运维手册 |

工作区与目录组织借鉴 `daeuniverse/honk`，不复制其代理或 eBPF 业务。

### src 子目录规则

- `apps/*/src`、`crates/*/src` 根只保留必要的薄 `main.rs`/`lib.rs`；它们只做模块声明、重导出、
  参数装配和进程生命周期，不放领域规则、SQL、HTTP 处理器、组件或外部调用。
- 实现放入有单一 owner 的职责目录，即使只有一个文件也如此；目录可用 `mod.rs` 作入口。
  使用 `domain/`、`ports/`、`services/` 等明确名称，禁止 `misc/`、`common/` 兜底或巨型 `mod.rs`。
- 同目录三个及以上同前缀实现文件默认表示缺少子目录；必须拆分、消除伪分组命名，或在规范中记录例外。
- 新增或移动源码时同步模块 README、架构 owner 和相关迁移阶段说明。

## 技术栈

工具链、MSRV、依赖版本与理由集中在[技术栈](docs/design/tech-stack.md)；精确解析以 `Cargo.lock` 为准。
使用 Rust edition 2024、Tokio、Axum、SQLx、Dioxus CSR、Fluent、reqwest/rustls 和 TOML 配置。

构建工具链跟随 stable 并由仓库固定，CI 独立校验 MSRV；只有上游正式声明的版本才能称为 LTS，
不能把 MSRV 当作 LTS。本地与 CI 的 Cargo 静态检查使用 `--locked`；升级核对 MSRV、WASM 和各已实现后端。
不为尚未使用的能力引入程序库，不启用 Dioxus fullstack/server 或第二套 HTTP/前端构建链。

## 依赖方向与模块所有权

装配根注入适配器，应用层依赖窄端口及领域，依赖方向向内。完整 owner 表见[架构](docs/design/architecture.md)。

| owner | 必须守住的边界 |
| --- | --- |
| `auction-domain` | 仅领域安全依赖；不导入 Axum、Dioxus、SQLx、reqwest，不读环境、初始化日志或访问外部服务 |
| `auction-application` | 用例、事务编排、应用错误和窄端口；不读进程配置、不监听、不直接执行 SQL/外部调用 |
| `auction-contracts` | 稳定输入输出、流 DTO 和错误；数据库记录不能直接成为浏览器 DTO |
| `auction-storage` | 实现应用端口，拥有 SQL、池、事务、锁和后端差异；业务决定留在内层 |
| `auction-cache` | 可丢弃、可重建适配；miss、不可用或失效失败回源，不改变命令结果 |
| `auction-integrations` | 外部传输与认证；响应经应用命令或持久化 Worker 处理，不直接改拍卖状态 |
| `auction-http` | 输入/命令与输出/DTO 映射；不包含竞价、结算或状态机规则 |
| `auction-web-public` / `auction-web-admin` | 各自页面、路由、加载、交互及译文；不互相依赖，不导入 SQLx、数据库模型或集成客户端 |
| `auction-web-shared` | 真正通用、无业务偏向的基础能力，不共享业务大结构体 |
| `auction-jobs` | 调度、重试、游标与 Worker 编排，不建立第二套状态机 |
| `auction-config` | TOML 解析和校验；业务模块不得散落环境读取 |
| `auction-setup` | 无配置首次设置、一次性令牌与初始化门禁；正式 Admin 不挂载设置写接口 |
| `auction-observability` | tracing、metrics、健康与关联 ID；敏感值不得进入日志 |
| `auction-process` | 信号、有界 drain 和 abort-on-drop 守卫；不读配置、监听、发心跳或决定业务停止顺序 |
| `apps/*` | 参数/配置、依赖注入、绑定、优雅关闭及退出码 |

设置服务默认 loopback。合并入口只有显式精确 HTTPS origin 与受信代理 IP 才允许远程设置，
不得信任任意转发头；配置生成仍属 `auction-config`。

### 模块化工作方式

功能按“领域 → 用例/端口 → 存储/外部适配 → HTTP/Web/Worker”落地。每个模块、表、事件、路由、
配置字段和写入查询只有一个 owner；保持函数小而内聚，禁止巨型模块、跨层条件和私有持久化访问。
历史格式通过防腐层/映射兼容，不把旧表或 wire 命名传播到新领域。共享能力须稳定且无业务偏向。

## 领域不变量

修改写入、状态迁移、结算或后台任务时，在代码或变更说明中明确保护的不变量。
[拍卖状态机](crates/auction-domain/src/auction/mod.rs)只是领域的一部分；详细语义见[不变量契约](docs/contracts/invariants.md)。

1. 可变命令使用稳定 `command_id`；同 ID/同负载返回原结果，同 ID/不同负载冲突。
2. 在同一后端事务内锁定拍卖、标的和有效授权，校验资格/状态并计算结果。PostgreSQL 竞价及关键
   生命周期使用 Serializable、明确行锁；序列化冲突最多尝试 3 次（含首次）。队列领取等路径才可用 Read Committed
   加显式锁；禁止跨事务先读价后写入。
3. 成功竞价的事实、版本、追加审计和持久事件/outbox 必须同事务写入。
4. 论坛、邮件、通知等外部写入只在本地提交后执行；外部失败不能抹掉已接受竞价。
5. Worker 游标和命令账本必须持久化，不能只靠内存定时器或游标支持恢复。
6. 非法迁移、金额、编码、身份或过期版本失败关闭，不静默修正。
7. 会话凭据、论坛 Cookie、邮件 Cookie 等密钥材料不得出现在日志、DTO、快照或提交的配置中。
8. 审计只追加，更正形成新事实，不能编辑历史证据。
9. 缓存不得决定金额、资格、幂等、锁、会话或审计；禁用或丢失缓存不破坏正确性。

## 命名规则

- 模块、文件、函数、变量、字段用 `snake_case`；类型、trait、别名及 Dioxus 组件用 `PascalCase`；
  常量/静态值用 `SCREAMING_SNAKE_CASE`。缩写统一为 `HttpClient`、`ApiError`、`AuctionId` 等 Rust 风格。
- 业务标识带上下文，如 `auction_id`、`lot_id`、`command_id`，避免含混的 `id/data/item`。
- Dioxus props 用 `#[component]` 参数；事件和状态命名表达用户动作。
- 新 URL 用小写连字符，JSON/TOML/SQL 名用 `snake_case`；已发布路径、事件名和字段按契约保留，
  历史命名通过 serde 映射。
- CSS 使用小写连字符和 BEM，例如 `public-auction-catalog__name`、`public-auction-catalog--compact`。

## 中文注释与文档要求

模块职责、公共 API、领域不变量、事务/锁/幂等策略、历史契约映射、外部服务限制，以及不明显的安全或性能取舍都需要注释。
新增/修改注释默认中文：模块用 `//!`，公共类型、函数和端口用 `///`；复杂分支用 `//` 解释原因、
不变量、锁/幂等、兼容、安全或性能取舍，删除逐行复述、重复或过期说明，保留必要契约和取舍。触及的英文注释翻译为中文，不作无关批量改写。
协议、API、SQL 关键字及必须匹配的字符串保留原文。新增/修改的 README、架构及运维正文使用中文。

## 代码风格

- 使用仓库 rustfmt，提交前 `cargo fmt --all -- --check`；Clippy 使用 `-D warnings`，不得关 lint
  掩盖设计问题。必要例外限最小范围并用中文解释。
- 函数单一职责，优先早返回、明确错误和清晰导入；库用 `thiserror`，只有装配根可用 `anyhow::Context`。
- 请求、Worker、持久化路径避免 `unwrap/expect`；不可恢复启动不变量须说明并给出可行动错误。
- 使用结构化 tracing 字段；消息可为稳定英文事件名，禁止完整凭据、密码、Cookie 或密钥。
- DTO、数据库记录、领域聚合分离，在边界映射；配置不散落于环境变量。
- CSS 两空格缩进，使用共享 `style/base.css` 变量；surface 专属规则留在各自 `assets/`。
- 只做需求相关的最小修改，不顺手重构或改名。

## 配置规则

权威字段、默认值和示例见[配置说明](ops/config/README.md)。变更字段同步解析、示例、相关 README 和部署说明。

- 运行配置一律 TOML；`AUCTION_CONFIG_FILE` 只选择角色文件，不覆盖单项设置。
- 三角色必填 `common_file` 指向同一非敏感公共文件，分别必填 `secrets_file`；相对路径以角色文件所在目录解析。
  公共文件拥有日志过滤器、指标、缓存和拍卖策略；角色文件拥有 listener、tracing、realtime、会话或 Worker 调度。
- 三角色任一配置 `runtime_config_file` 时，部署预检要求全部引用同一文件并保持装备标色规则快照一致；Public 只应用
  装备分组规则。`operations` 六组仍由 Admin/Worker 接管新拍卖经济模板、自动排期、论坛同步、告警、Worker 循环
  及邮件预算；`managed` 默认关闭。启用接管时另校验有效模板/排期一致。
- runtime 保存后重启相应进程生效，不回写拍卖快照、任务游标或租约。MoogleMail transport、凭据、mailbox identity
  和全局启用仍由部署配置拥有；省略 `operations.mail` 保持旧指纹，cron 校验/保存/启动复用配置解析器。
- `[database]`、`database_url`、`redis_url`、Cookie、`admin_actor/admin_secret` 只进敏感 TOML。
  Admin 凭据成对配置，secret 至少 16 字节。敏感文件不提交，只允许运行账号读取；错误不得回显 TOML 原文或敏感值。
- PostgreSQL 优先用 `[database]` 必填 `host/port/username/password/name`；端口非零，账号、密码和库名按原文生成 URL。
  旧 `database_url` 仅接受 `postgres://`、`postgresql://`，不能与 `[database]` 同时提供。
- MoogleMail 本地联调仅允许 `auction-config::load_mooglemail_keyenv` 严格读取 `.keyenv` 的 `hv-ck-key1/2`。
  Worker 只用 `hv-ck-key1`；`hv-ck-key2` 仅供显式隔离验证，不作候选或自动切换。论坛与游戏邮件每种 Cookie 全局最多一条 active。
- 旧的单文件敏感配置必须明确失败，不暗中兼容或引入通用环境覆盖。
- 受版本控制 TOML 每个赋值字段前须紧邻中文注释，逐项标“可选/必填”、默认值或启用条件。

## 数据库与迁移

- 新持久化能力先登记[架构矩阵](docs/design/architecture.md#部署档位与能力)并对照[schema owner](docs/contracts/schema.md)审查。
- PostgreSQL 固定 major 18 并跟进修复版，major 升级单独评审迁移/回滚。
- 驱动由显式 `postgres` feature 启用，不使用 `AnyPool` 隐藏池、事务和锁语义。
- 迁移只追加，已发布文件禁止修改或删除。
- 每张表、迁移、查询、事件和路由只有一个 owner；兼容历史结构使用防腐层，不复制未经审查的旧表。
- SQLx 宏具备可复现 PostgreSQL schema/check 路径；离线校验只更新受影响元数据。
- 破坏性操作须有手册、精确目标确认与可验证备份，开发脚本不得随意加入 `DROP DATABASE`。

## Web 规则

- Public/Admin 独立 Dioxus CSR SPA、路由、译文与资源；不启用 SSR/hydration、fullstack/server。
  改用 Askama + HTMX 等方案前，必须由用户明确放宽浏览器全 Rust、禁止 JavaScript 的契约。
- 业务逻辑留在 Rust，不引入 React、TypeScript、Node/npm 依赖或第二套构建链；dx 按需下载 esbuild
  仅作构建工具是唯一例外，不得成为运行时依赖。
- `just web-build` 使用 `dx bundle --platform web --profile wasm-release`，复制至稳定目录后分别内嵌。
  保留 `allow_missing`：无产物仍可编译，运行返回明确 503，不 panic。
- 只有显式 `web-embed` 才选择资源；API-only 固定空集合，不由构建机残留 `target/dx` 决定。公开制品不得含管理资源。
- 核心竞价、认证及管理命令保留版本化 REST/JSON；Public 实时使用 sequence/version WebSocket，跳号/重连通过
  HTTP Catch-up 与权威快照恢复。SSE 尚无 endpoint，新增前冻结独立契约。
- 浏览器只消费 `auction-contracts` DTO，不接收 SQL 行、数据库模型或服务端密钥。变更竞价/管理状态的动作携带
  `command_id`，明确展示幂等及冲突结果；不得用示例业务行伪装权威状态，演示空态明确标注。
- 所有界面文案经本 surface 的 `dioxus-i18n` / `i18n/*.ftl`，不得组件硬编码。支持 `en/zh-CN/zh-TW/ja/ko`，
  Public 默认 `en`，Admin 默认 `zh-CN`；新增消息同步五份，缺键会 panic。Fluent ID 用连字符，不能含点号。
- 偏好键固定 `hv.lang`，非法值退回对应默认语言；共享层只持有语言集合、持久化和 `<html lang>` 同步。
- 业务表占容器 100%，组件按内容/时间/动作独立定义完整百分比列宽；Markdown 按内容权重计算，见[表格契约](docs/design/table-layouts.md)。
  窄屏保持表头和行结构，不用固定最小宽度、横向滚动或卡片化。长文本截断并提供全文提示，复杂文本支持键盘/触控查看。
- 保持既有 CSS、结构与视觉契约；样式或静态产物完成不等于业务验收。

## 默认验证与测试范围

默认只做格式、编译与静态分析，命令以 [Justfile](Justfile) 和[开发指南](docs/guide/development.md)为准：

```bash
just verify
# 单项：fmt / check / feature-check / lint / wasm-check
# 策略门禁：bash ci/checks/check.sh repository layout config i18n
```

`cargo check --locked --workspace --all-targets` 只编译已有目标，不运行测试。除非用户明确要求，
不得新增或执行单元、集成、功能、端到端、浏览器测试、功能测试脚本及手工冒烟；已有测试不删除、不扩展。
恢复行为验证前说明范围与成本，必须覆盖 PostgreSQL 契约、隔离及行锁语义。

## 运行进程

`apps/*` 默认 feature 不含数据库驱动；运行时明确选 `postgres`，需页面时先构建 Web
并选 `web-embed`。Public/Admin 默认监听 `127.0.0.1:8090/8091`，不可共用入口。
真实敏感配置、迁移、首次设置和启动命令见[开发指南](docs/guide/development.md)及[部署手册](ops/README.md)。
启动服务、访问健康接口或功能流程属于行为验证，不是默认静态门禁。

## 变更与 Git 纪律

- 修改前 `git status --short`，保留无关改动；未经授权不得 `git reset --hard`、`git checkout --` 等破坏性操作。
- `AGENTS.md`、`.gitignore`、`.editorconfig`、全部 README 和 `Cargo.lock` 纳入版本控制，不得忽略项目契约；
  `CLAUDE.md` 仅为忽略的本地工具文件。
- 不提交凭据、带密码 URL、私有 TOML、本地环境、`target/`、临时制品或机器专属绝对路径。
- 路由、状态码、字段、事件、配置选择器、迁移或制品变化同步对应契约。提交标题可用 `type(scope): summary`。

## 文档维护

从[文档导航](docs/README.md)进入全部正式文档。每项事实在所属 owner 文档维护，其他页面用摘要和链接；
设计理由与限制直接归入正式文档，不新增独立笔记目录或技能格式模板。Public/Admin 设计分开维护。
[状态页](docs/status.md)区分当前事实和历史证据，Git 保留变更历史；不得改写过去的验证结论。
文档没有自动门禁：链接一律写成以仓库根为起点的相对路径（如 `docs/contracts/schema.md`），移动或重命名文件时人工核对全部引用与锚点。文档维护不得暗改 wire、迁移、安全策略或测试范围。

## 交付工作流

1. 先明确路由、JSON、事件、配置、数据库、部署及能力档位契约。
2. 在领域定义规则，在应用建立用例/端口；建立 PostgreSQL 锁、幂等、审计和 outbox。
3. 接入存储/外部适配、HTTP、Dioxus 与持久化 Worker；不复制状态机或绕过 owner。
4. 同步契约、模块说明、设计与运维文档，执行适用静态门禁。
5. 用户明确要求后才开展隔离环境、真实外部服务或切换前行为验证，登记可追溯证据及限制。

源码定位时，仓库存在 `.codegraph/` 就先用 `codegraph_explore` 或 `codegraph explore`；
未索引的文档/配置可直接读取。每次交付明确模块 owner、外部契约及实际验证范围，不以骨架可编译宣称验收。
