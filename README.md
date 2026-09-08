# Auction Rust

这是一个全 Rust 的论坛生态拍卖系统工作区。业务规则、HTTP/事件契约、数据语义和部署
边界均由本仓库的源码、迁移与文档共同定义，不依赖仓库外的实现或源码树。

## 设计目标

- 保留 Public、Admin、Worker 三个可独立部署的运行边界。
- 让竞拍规则、状态迁移、幂等和结算不依赖 HTTP、数据库或前端框架。
- 公开端和管理端的 Web UI 也使用 Rust：Dioxus 客户端渲染，产物内嵌进后端二进制。
- 界面原生支持中（简/繁）、英、日、韩五种语言；公开端默认英文，管理端默认简体中文。
- 将论坛、MoogleMail、数据库、日志和配置视为可替换适配器。
- Redis 缓存是可选适配器，默认不编译、不启用；关闭或故障时回退到权威数据源。
- PostgreSQL 提供完整生产能力；SQLite 作为单主机、低并发 Lite 替代品，只实现明确列入
  Lite 范围的功能。两个后端已支持的竞价写入都必须保证事务、幂等和审计一致性。
- 保持本仓库已经冻结的拍卖页面视觉语言，并在 Rust Web 层交付完整交互和业务数据。

## 当前状态

当前已进入功能与运行证据收口阶段，不再只是启动骨架。本仓库登记的 feature API 为 Public
21 条、Admin 43 条，其中 A38-A43 是独立登记的 Admin 能力；浏览器路由为 Public 15 条
（含独立论坛同步页）和 Admin 21 条。
PostgreSQL 完整档也已建立竞价、代理、账户、管理命令、
审计、实时、结算/recovery、凭据、MoogleMail 和论坛的主要 owner；SQLite Lite 只实现已声明的
单主机能力，完整档端点在 Lite 部署中明确报告不支持。路径和模块存在不代表生产验收完成：
参与者资格锁、黑名单副作用、慢速代理、生命周期锁序、实时 wire、兼容竞价账本、Admin mutation
认证顺序、Public 身份化读取、结算重放不变量和若干 HTTP/WebUI/配置边界已经静态收口；部分 HTTP、
竞价、结算、灾备和浏览器路径已有隔离环境局部 R1，组合输入、并发、损坏事实和真实外部服务仍需
各自的运行证据。

runtime 配置更新和恢复在同一跨进程文件锁下完成读取、文件替换及账本提交，并在替换后采用向前
恢复，避免并发旧请求覆盖后续配置或提交确认丢失时把文件回退到旧版本；缓存操作
具备总超时与并发上限。公开竞价日志选择器支持完整分页，两端页面监听在卸载时释放。PostgreSQL
竞价保留 Serializable 与 lot 排他锁，同时以共享锁保护只读世界/拍卖门禁；价格历史编号页在同一
快照内复用筛选结果供分页与统计读取。这些改动的静态验证不替代并发、故障和浏览器运行验收。
MoogleMail 批次拉取与处理默认使用 600 秒总预算，按旧到新处理积压邮件，预算到期时只原子提交
已完成前缀及其游标；对应参数可通过部署 TOML 或 Admin runtime 邮件配置管理。
每次拉取将同一绝对截止时刻传到 gateway，详情按预算返回连续前缀；处理前核验持久化完成证明，
取件、退回和入站通知则等待该证明提交后才可发送，防止重读已变化邮件产生新的业务副作用。

路由和 owner 齐全不等于运行边界已收口。三进程现已通过共用生命周期原语统一处理
SIGINT/SIGTERM：首次信号触发 10 秒有界 drain，Public/Admin 停止接收新 HTTP 连接，Worker
停止领取新任务并完成当前有界批次，随后停止心跳并按配置限时刷新 tracing；排空期间第二次信号
会强制收口。Public/Admin 现已接入统一 HTTP 边界：请求 ID 与命令审计关联、W3C trace context、
访问日志、panic recovery、安全响应头、处理/请求头超时、可信代理解析和有界限流器均已落地。
显式 Public/Admin 浏览器路由、未知 API/health 的 JSON 404、未知页面 404 和已冻结的读响应缓存策略也已
恢复。三进程心跳现在每轮复用数据库/schema readiness，读取进程内审计写入失败饱和计数和各自的
tracing 安全状态，不再发布固定占位值。Admin runtime 的人工提前开放和装备来源策略也已冻结到
启动装配，并分别在锁内生命周期事务和受限流式 HTTP 读取中生效。浏览器 HTTP/stream DTO 已统一
由 `auction-contracts` 拥有；Public/Admin 的 `RustEmbed` 类型也已分别移入对应装配根，并由显式
`web-embed` feature 区分 API-only 与内嵌 Web 制品，任一部署制品都不会引用另一端资源。
Public/Admin 已支持共享安全 TCP/Linux Unix listener，`auctionctl`
已提供稳定退出码和脱敏有效配置诊断；HTTP、配置、Worker、Web surface 与 storage 入口也已按
职责使用具名实现文件。canonical PostgreSQL v28 历史格式的隔离 backup/adopt/rebuild/restore 链
已经取得 R1；当前剩余工作主要是并发/损坏输入、非空生产数据、真实外部服务、部署切换和完整
浏览器证据，不能据此宣称已经完成生产验收。

MoogleMail 已覆盖远端 HTML 解析、持久化轮询、拍卖/用户入站、send/return/take/notify、
Sent/Balance 证据和 delivery outbox；recovery 链覆盖五类已登记 action。Public 已建立持久化
Catch-up、进程内有界 WebSocket Hub 和权威快照恢复组件；v1 逐帧单事件 wire、握手后 HTTP 补齐、
4 KiB/50-60-5 秒连接边界、稳定关闭原因、discovery 专属容量与 settled 新连预检已静态恢复；真实帧
R1 仍待补录。Admin 页面均已连接实际 API，realtime/delivery 嵌套分页、严格成功 decoder、稳定错误
反馈和客户端未知路由的认证外壳也已恢复；对应损坏响应与认证组合仍待 R1。`auctionctl` 还提供
配置/迁移检查、systemd 三进程顺序控制、部署预检、retention 只读盘点和 PostgreSQL 备份、
离线校验、默认只读且需要双重确认的数据库重建、空库恢复及回执。canonical PostgreSQL v28
历史格式另由专用 backup/verify/adopt 链路接管，普通迁移和恢复不会猜测未识别的 schema；数据库
之外的五语言规则目录由显式 `import-rules-directory` 命令导入，不会用硬编码 seed 或覆盖
不同的 Admin 已发布正文。
MoogleMail 异常路径还会按独立运维契约保存加密原始响应或实际提取的邮件字段；普通成功邮件、日志、数据库和
DTO 不保留正文，SQLite Lite 不新增该能力。离线证据导出使用 `auctionctl mail-evidence export`，具体边界见
[`docs/operations/mail-diagnostics.md`](docs/operations/mail-diagnostics.md)。
MoogleMail 的标准 cron/时区、请求间隔和请求超时属于当前可配置运行契约；Worker control 使用
安全 runtime 目录、实例锁、固定 group/权限和显式 UID/GID allowlist。部署预检会校验
Admin/Worker control socket、三组数据密钥和实际 TCP/Unix listener 拓扑。

尚不能据此宣称生产验收完成：真实 MoogleMail/E-Hentai、非空生产 PostgreSQL 数据、目标部署切换
和 Public/Admin 浏览器产物仍缺运行态证据。start/stop/restart/status 已由 Rust `auctionctl` 与
systemd owner 边界实现；destructive rebuild 也已成为独立、默认只读且需要备份/双重确认/回执的
显式命令，不会进入启动路径。外部 realtime 广播 sink 和 retention mutation 均不属于当前已冻结
能力；若要新增，必须先建立独立契约和授权边界。

“全 Rust”只约束本仓库实现及其程序库依赖。PostgreSQL、SQLite、可选 Redis、反向代理
等外部组件可以使用各自的官方实现，不要求它们由 Rust 编写。

除非用户明确要求，本项目暂缓新增或运行单元测试、集成测试、功能测试用例、端到端测试
和浏览器测试。已有测试文件保留，不因本规则删除。

## 目录约定

```text
apps/                         # 薄进程入口，负责装配依赖和管理生命周期
├── public-api/               # 公开端 API 与公开 Web
├── admin-api/                # 管理端 API 与管理 Web
├── worker/                   # 拍卖生命周期、论坛和通知任务
└── auctionctl/               # 数据库、配置和运维命令行工具
crates/                       # 可复用能力，按模块所有权拆分
├── auction-domain/           # 纯领域模型与不变量
├── auction-application/      # 用例、端口和事务边界
├── auction-contracts/        # HTTP/WebSocket DTO 与错误契约
├── auction-http/             # Axum 路由与 HTTP 表面适配器
├── auction-web-shared/       # 浏览器端共享设计变量、基础组件与语言选择
├── auction-web-public/       # 公开端 Dioxus 界面（15 条路由，默认英文）
├── auction-web-admin/        # 管理端 Dioxus 界面（21 条路由，默认简体中文）
├── auction-storage/          # PostgreSQL 完整适配器与 SQLite Lite 适配器
├── auction-cache/            # 默认关闭的缓存边界与可选 Redis 适配器
├── auction-integrations/     # E-Hentai、MoogleMail 等外部网关
├── auction-jobs/             # Worker 任务编排
├── auction-config/           # TOML 解析与运行配置
├── auction-observability/    # tracing、metrics 和健康检查约定
└── auction-process/          # SIGINT/SIGTERM、有界排空和任务所有权原语
docs/                         # 架构、技术选型与版本化契约
migrations/                   # PostgreSQL 完整 schema 与 SQLite Lite 子集的追加式迁移
ops/                          # 配置样例、部署和恢复文档
```

这种结构借鉴 Honk 的 Cargo 工作区、`crates/*` 能力拆分、根目录工程约定和独立文档/运
维目录，但不会照搬 Honk 的代理业务模块。

每个 apps/*/src 和 crates/*/src 还必须按职责划分子目录；根 main.rs/lib.rs 仅保留薄装配
入口，实际领域、适配器、服务和界面源码放在带明确 owner 的子目录中，禁止把实现文件平铺
在 src/ 根目录。

该约束由 `just layout-check` 静态检查，并作为 `just verify` 的组成部分。

## 本地开发

需要固定的 Rust 1.98.0 当前稳定工具链和 `wasm32-unknown-unknown` 编译目标；workspace
MSRV 为 1.94.1。Rust 官方没有 LTS 且只支持最新 stable；Tokio 使用官方 `1.51.x` LTS，
其他无 LTS 的 crate 由 `Cargo.lock` 固定并受控升级。运行时配置统一使用 TOML；
`AUCTION_CONFIG_FILE` 只用于选择角色配置文件，不用于覆盖单个配置项。Public、Admin 与 Worker
角色文件都通过 `common_file` 引用同一份公共非敏感 TOML，并通过 `secrets_file` 引用独立敏感
TOML；数据库连接串、Redis 连接串、Cookie 和会话签名密钥
只放在该敏感文件中。Admin 与 Worker 主配置还可通过 `runtime_config_file` 指向同一份由 Admin
管理的非敏感 runtime TOML；A26/A27 的 PostgreSQL 完整档更新会写入该文件，并要求重启受影响的
Admin/Worker，SQLite Lite 不提供更新命令。Worker 对 MoogleMail 只消费其中的 world、发件人、装备过滤和通知
模板业务区块，其 transport、凭据、邮箱身份与全局启用开关仍由 Worker 主配置拥有。runtime 的
`operations` 另提供新拍卖默认策略、自动排期、论坛同步、运维告警、各 Worker 循环以及邮件调度/处理参数；每组
`managed` 默认关闭，勾选后才在对应进程重启时接管。新经济模板仅用于之后创建的拍卖，完整字段与
部署要求见 [`ops/config/README.md`](ops/config/README.md)。
拍卖策略模板位于公共配置的 `[auction]` 区块；Admin 创建拍卖时会把已校验模板冻结到拍卖策略快照，
具体字段和默认值见 [`ops/config/README.md`](ops/config/README.md)。

构建浏览器端产物还需要 dx CLI（`cargo install dioxus-cli --locked`）。dx 会按需从 npm
注册表下载 esbuild 作为构建期工具，它不是运行时依赖，也不引入 Node 运行时或第二套前端
构建链。后端使用 `allow_missing` 内嵌产物，因此 API-only 或未构建前端时后端仍可编译，只是
静态资源路由返回 503；是否嵌入不再取决于构建机上是否恰好残留 `target/dx` 文件。

```bash
# 使用 SQLite Lite 示例配置（本地开发/单节点低并发）
AUCTION_CONFIG_FILE=ops/config/examples/public.toml \
  cargo run -p auction-public-api

# 构建两个 surface 的前端产物（需要 dx CLI）
just web-build

# 构建 API-only 与包含前端资源的 PostgreSQL 完整档制品
just build-api
just build-release

# 核对实际二进制中的 surface 隔离和 Web payload 哈希
just verify-artifacts

# 干净工作树上的完整构建、门禁与确定性发布 archive
just release-check

# 完整生产档在不提交的敏感文件中使用 postgres://... 连接串
just verify
```

受版本控制的 `secrets.toml` 只含安全占位值。实际部署应把它复制为被 Git 忽略的
`*.secret.toml`，限制文件权限，并让主配置的 `secrets_file` 指向该文件。Redis 还需要以
`cache-redis` feature 构建对应进程；仅填写 `redis_url` 不会把 Redis 依赖带入默认制品。
Admin runtime 模板位于 [`ops/config/examples/runtime/admin.toml`](ops/config/examples/runtime/admin.toml)，
生产部署应复制到受信目录并设置为 `0640`；其中不得放入凭据或连接串。
三进程部署目录、启动顺序、权限、Caddy 和 systemd 模板见 [`ops/deploy/README.md`](ops/deploy/README.md)。
发布命令只装配 PostgreSQL 完整档的四个二进制、受批准的配置/部署模板、依赖摘要、Web payload
清单和 SHA-256；正式 package 拒绝脏工作树，本地迁移取证必须显式传入 `--allow-dirty`，并在名称和
摘要中保留 `dirty` 标记。
发布构建在 Web/native 编译前冻结源码身份，并在各阶段及装包发布前核对；期间发生源码修改或
提交会使门禁失败，需重新构建，不能把旧产物标记成新提交。

`just verify` 默认只执行格式化、源码布局/命名、配置示例、五语言 Fluent 键集合门禁、锁定依赖的
编译检查、Clippy 和 WASM 静态检查，不执行测试。现行设计与交付边界见
[`docs/architecture.md`](docs/architecture.md)、[`docs/tech-stack.md`](docs/tech-stack.md) 和
[`docs/contracts/`](docs/contracts/) 下的路由、事件、数据库对象与领域不变量清单。
