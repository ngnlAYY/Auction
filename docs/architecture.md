# 架构

[docs/README.md](README.md) · [docs/business-rules.md](business-rules.md) · [docs/development.md](development.md) · [ops/README.md](../ops/README.md)

HV Auction 是 HentaiVerse 装备拍卖服务：玩家通过网站与论坛出价，通过游戏内 MoogleMail 寄售装备、付款与收货。系统由 Rust 2024 工作区构成，后端为三个可独立部署的进程（Public、Admin、Worker）加一个运维 CLI，前端为两个 Dioxus WASM 单页应用，唯一的业务数据库是 PostgreSQL 18。原始邮件诊断证据另以加密文件保存，运行配置与备份回执也位于文件系统。

## 进程模型

```text
                 浏览器（玩家）                     浏览器（运营）
                      │ HTTPS                           │ HTTPS
                反向代理（Caddy 等）              反向代理 / SSH 隧道
                      │                                 │
        ┌─────────────▼─────────────┐     ┌─────────────▼─────────────┐
        │ auction-public-api        │     │ auction-admin-api         │
        │ 公开 API + WebSocket       │     │ 管理 API + 首次设置        │
        │ realtime relay            │     │ runtime 配置写者           │
        └─────────────┬─────────────┘     └──────┬──────────────┬─────┘
                      │                          │ Unix socket  │
                      │                          │（Worker control）
                      │                    ┌─────▼──────────────▼─────┐
                      │                    │ auction-worker           │
                      │                    │ 生命周期/结算/履约/投递/  │──► MoogleMail（HentaiVerse）
                      │                    │ 邮件/论坛/恢复/告警        │──► E-Hentai 论坛
                      │                    └─────────────┬────────────┘
                      └───────────────┬──────────────────┘
                               PostgreSQL 18（唯一事实来源）
```

| 进程 | 职责 |
| --- | --- |
| [apps/auction-public-api/README.md](../apps/auction-public-api/README.md) | 玩家读接口、竞价/代理/标记命令、登录会话、WebSocket 推送；内嵌 Public 页面 |
| [apps/auction-admin-api/README.md](../apps/auction-admin-api/README.md) | 管理接口与 Admin 页面；runtime 配置文件的唯一写者；分体部署的首次设置 |
| [apps/auction-worker/README.md](../apps/auction-worker/README.md) | 所有后台循环与外部集成；本机 control socket |
| [apps/auction-server/README.md](../apps/auction-server/README.md) | 可选：单一可执行文件，以子进程监管上述三者 |
| [apps/auction-cli/README.md](../apps/auction-cli/README.md) | 运维：配置检查、分体部署编排、迁移、备份恢复、业务运维 |

进程之间不直接调用业务接口：它们通过数据库共享事实。唯一的进程间通道是 Worker control（Unix socket，peer 凭据授权），用于就绪探测、状态读取与手动触发邮件轮询。

## 分层

```text
apps/*（装配根）
   │  读取配置、连接数据库、选择适配器、启动运行时
   ▼
适配器：auction-http · auction-storage · auction-integrations · auction-jobs · auction-setup
   │  实现端口
   ▼
auction-application（端口 + 用例）
   ▼
auction-domain（纯领域规则）
```

| crate | 层 | 说明 |
| --- | --- | --- |
| [crates/auction-domain/README.md](../crates/auction-domain/README.md) | 领域 | 拍卖状态机、竞价/代理规划、结算计算、义务与资金规则、装备词典；无 I/O |
| [crates/auction-application/README.md](../crates/auction-application/README.md) | 应用 | 端口（trait）、命令、安全投影与用例编排；不依赖具体数据库或网络实现 |
| [crates/auction-storage/README.md](../crates/auction-storage/README.md) | 适配器 | PostgreSQL 实现、迁移执行、运维数据库能力 |
| [crates/auction-integrations/README.md](../crates/auction-integrations/README.md) | 适配器 | MoogleMail 与 E-Hentai 客户端、论坛渲染/解析、装备来源、加密信封 |
| [crates/auction-http/README.md](../crates/auction-http/README.md) | 适配器 | 两个 HTTP 表面、安全边界、会话、WebSocket Hub、内嵌资源 |
| [crates/auction-jobs/README.md](../crates/auction-jobs/README.md) | 适配器 | Worker 调度运行时与 control 服务端/客户端 |
| [crates/auction-setup/README.md](../crates/auction-setup/README.md) | 适配器 | 首次部署设置服务 |
| [crates/auction-config/README.md](../crates/auction-config/README.md) | 基础设施 | TOML 配置加载、合并与校验，runtime 文件存储 |
| [crates/auction-observability/README.md](../crates/auction-observability/README.md) | 基础设施 | 日志、指标、tracing、心跳 |
| [crates/auction-process/README.md](../crates/auction-process/README.md) | 基础设施 | 信号订阅、有界排空、任务守卫、本机 readiness 探针 |
| [crates/auction-contracts/README.md](../crates/auction-contracts/README.md) | 契约 | 前后端共用的 HTTP/WebSocket DTO（可编译到 wasm32） |
| [crates/auction-web-shared/README.md](../crates/auction-web-shared/README.md) | 前端 | 两个前端共用的浏览器机制、组件与设计 token |
| [crates/auction-web-public/README.md](../crates/auction-web-public/README.md) / [crates/auction-web-admin/README.md](../crates/auction-web-admin/README.md) | 前端 | 公开站点 / 管理后台（Dioxus 0.7 CSR） |

### 共享 helper 归属

跨装配根重复的机制按下表收敛到 owner，调用方只做配置解析、绑定与生命周期，不在 `apps/*` 复制实现或在其上叠加业务规则：

| helper | owner | 调用方与边界 |
| --- | --- | --- |
| `HttpListener`（TCP/Unix）、`serve_http`、`UnixHttpListener` | `auction-http` | Public/Admin 的 `bootstrap/listener` 只解析监听配置并绑定；连接服务、Unix socket 实例锁与 `serve` 描述共用同一实现 |
| `serve_metrics`、`spawn_metrics_listener` | `auction-http` | 三个装配根把附加指标出口挂在自己的 `TaskGuard` 下；只持有同一个 `ProcessMetrics`，不引入业务 state、control 或资源路由，绑定/服务失败只降级采集 |
| `probe_ready`、`ReadinessTarget` | `auction-process` | `auction-cli` 的服务编排与 `auction-server` 的子进程监管各自从配置推导端点与路径，状态行判定只有一份；探针不读配置，也不以 PID 存活代替就绪 |
| `shutdown_tracing` | `auction-observability` | 三个装配根须先完成业务排空再调用；第二个终止信号直接结束进程，避免可选 exporter 拖延 systemd 停止 |
| `support/files`、`support/process` | `auction-cli`（应用内私有） | 备份、部署与数据库命令共用的私有文件/锁文件/原子发布边界，以及有界 stderr 的子进程捕获与临时文件清理；不是跨 crate 公共 API |
| `wire`、`storage`、`timer` | `auction-web-shared` | 两端 decoder 共用 JSON 安全整数边界与严格键集合谓词；`storage` 容错读写 `localStorage`，非 WASM 构建恒为空操作；`timer` 是唯一的一次性定时等待，释放即取消。业务键名与业务校验仍归各 surface |

每项细节由对应模块 README 拥有：见 [crates/auction-http/README.md](../crates/auction-http/README.md)、[crates/auction-observability/README.md](../crates/auction-observability/README.md)、[crates/auction-process/README.md](../crates/auction-process/README.md)、[apps/auction-cli/README.md](../apps/auction-cli/README.md) 与 [crates/auction-web-shared/README.md](../crates/auction-web-shared/README.md)。

依赖规则：

- 内部依赖向内：适配器依赖 application/domain，application 的内部依赖只有 domain；domain 不依赖其他内部 crate。各层仍可使用符合职责的第三方基础库。
- 前端的内部依赖只有 `auction-contracts` 与 `auction-web-shared`，不依赖任何后端 crate。
- Public 二进制不包含 Admin 前端，Admin 不包含 Public 前端，Worker 不包含任何前端；`auction-server` 不依赖 `auction-cli`。发布脚本用 `cargo tree` 检查这些边界。
- 能力缺失（未配置数据库、密钥或外部凭据）时端口为空，接口明确返回“能力不可用”，不伪造成功或空数据。

## 一致性模型

### 命令与幂等

需要命令账本的业务写入使用稳定 `command_id`：同一 ID 同一请求重放原结果，不同请求返回冲突。竞价与关键生命周期在一个 **Serializable 事务**内完成查重、锁定快照、领域规划以及事实、结果、审计和事件写入，序列化冲突与死锁有界重试。会话登录/退出、配置校验等接口不套用统一的 `command_id` 协议；其他存储写入按各自端口选择事务与锁，不能把 Serializable 概括为全库默认。未知结果的业务命令只允许原样重试。

### 追加式事实

接受的出价、代理转换、结算结果、补偿流水与冲正、审计与发送尝试证明等采用追加式事实；具体受触发器保护的表见[docs/contracts/database.md](contracts/database.md)。当前状态（lot 当前价、领先者等）由同一事务维护；撤销时间、租约与状态投影按各自契约更新，不属于整表不可修改的保证。

### Worker 的工作项

需要跨进程续办的调度、投递等工作持久化在数据库中，通过租约及 fencing 版本防止过期任务写回；内存计时器只负责唤醒。进程崩溃后的可重领条件由各类工作状态决定，不能把“租约到期”视为外部动作未发生。涉及外部写入（发邮件、发帖、发私信）的步骤先持久化请求身份与发送状态，再调用远端；结果未知时进入核对流程，未取得对应动作证明不得重发。

### 运行时独占

- 每个外部 Cookie（邮件、论坛）同一时刻只能被一个进程使用：Worker 以 PostgreSQL session advisory lock 取得运行时所有权，连接断开即失去，失去后进程排空并退出重启。
- 每个 Public 实例以自己的 `consumer_key` 独占 realtime 消费游标。

### 时间

领域规划不读取系统时钟，时间作为参数传入；竞价用例在锁定后从事务端口读取数据库时间。排期按冻结的 IANA 时区与本地钟点换算，不从当前配置反推历史。

## 关键数据流

### 出价

```text
浏览器 POST /api/v1/lots/{id}/bids {command_id, amount}
  → auction-http：会话、origin、限流、严格 JSON
  → CompetitionService：Serializable 事务
       锁定 lot → AuctionRules + LotBidState → CompetitionPlan（出价 + 代理反超 + 软截止）
       → 写 bids / 代理转换 / lot 投影 / 审计 / lot.bid_accepted 事件
  → 响应：领先状态与当前价
Public relay 轮询事件 → WebSocket 推送 → 各浏览器重读受影响的资源
```

### 投稿到结算

```text
玩家 MoogleMail（主题 auction）→ Worker 邮件轮询 → 读取装备 → 校验/过滤
  → 加入接收投稿的场次 + 加密装备 key → outbox：收取附件
排期：Draft → Preview → Live → Final（Worker 或 Admin）
所有 lot 截止 → Worker 结算：金额 + 义务
履约：义务 → outbox 邮件（COD / 交付 / 货款）→ Sent 证据 → 满足 → 解除后续义务
恢复：付款超时提醒、退件重寄、上限后退回卖家并拉黑
```

## 配置模型

每个角色一个 TOML 角色文件，引用共用的 `common.toml`（日志、指标、拍卖策略）、敏感文件 `secrets.toml`（数据库、Cookie、密钥，仅 owner 可读）与可选的 runtime 文件 `runtime/admin.toml`。运行期间由 Admin 编辑 runtime；Public、Admin、Worker 分别加载自己负责的投影。启动前校验配置，后续运行时更新另有校验与幂等门禁。详见[ops/config/README.md](../ops/config/README.md)。

## 安全边界

- Public 与 Admin 分别监听，建议只在 loopback 或 Unix socket 上监听并由反向代理终止 TLS；Admin 不应暴露到公网。
- Public/Admin 会话 Cookie 为 HttpOnly；Admin CSRF Cookie 可由脚本读取，前端通过会话 DTO 取得 CSRF token。受保护的 Admin 写请求要求 CSRF token；状态变更请求按路由校验 Origin allowlist；登录、写请求与实时连接按客户端地址限流。
- 敏感数据静态加密：外部 Cookie、装备 key、用户登录 key 各用独立的数据密钥（信封加密）保存，MoogleMail 诊断归档使用第四个独立密钥域；每个域是“当前写入密钥 + 保留解密的旧密钥”的密钥环，按信封 key ID 在本域解析，未知 ID 失败关闭。日志、指标与错误响应只包含安全分类与标识，不包含秘密或邮件正文。轮换流程见[ops/keys/README.md](../ops/keys/README.md)。
- 浏览器 CSP 的脚本来源限制为同源，同时为 Dioxus 解释器与 WASM 启用 `unsafe-eval` 和 `wasm-unsafe-eval`；内联样式通过 `style-src 'unsafe-inline'` 放行，不加载第三方资源；表单只能提交到同源（`form-action 'self'`），页面仍不能嵌入或被嵌入。
- 上述两处放宽是安全复核后的有意接受，不是遗漏：Dioxus 0.7 的 CSR 启动必须能编译 WASM，运行时解释器要用 `new Function` 执行事件处理，去掉任一项两端都无法加载，当前没有等价的能力开关；剩余风险由“只加载同源脚本、不引入内联脚本或第三方资源”收窄——`eval` 只能执行同源资源中已有的代码，利用它需要先取得同源脚本执行权，而那本身已是更严重的缺陷。内联样式无法执行脚本，风险限于视觉伪装，且页面只使用自身样式。升级 Dioxus 时重新核对能否移除这两项。
- HSTS 只在浏览器确实经 HTTPS 访问时发送，代理后只采信可信上游的 `X-Forwarded-Proto`；当前值不含 `includeSubDomains` 与 `preload`，因为本站可能只是共享父域的子域，递归强制兄弟子域 HTTPS 超出本服务的控制范围。判定顺序与逐项响应头见[docs/contracts/routes.md#响应外壳](contracts/routes.md#响应外壳)与[crates/auction-http/README.md#请求边界](../crates/auction-http/README.md#请求边界)。

## 部署形态

| 形态 | 说明 |
| --- | --- |
| 分体部署 | 三个 systemd unit，由同机的 `auction-cli service`（或 `auction-stack.service`）按 Worker → Admin → Public 顺序编排；支持按角色独立重启 |
| 合并部署 | 一个 `auction-server` unit，以子进程运行三个角色；首次设置经网页明确确认后初始化专用空库 |

两种形态使用相同的配置目录布局与同一个 `auction-cli`。详见[ops/README.md](../ops/README.md)。
