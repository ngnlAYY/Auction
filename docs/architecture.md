# 架构

[文档导航](docs/README.md) · [业务规则](docs/business-rules.md) · [开发指南](docs/development.md) · [运维手册](ops/README.md)

HV Auction 是 HentaiVerse 装备拍卖服务：玩家通过网站与论坛出价，通过游戏内 MoogleMail 寄售装备、付款与收货。系统由 Rust 2024 工作区构成，后端为三个可独立部署的进程（Public、Admin、Worker）加一个运维 CLI，前端为两个 Dioxus WASM 单页应用，唯一的持久化存储是 PostgreSQL 18。

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
                               （可选 Redis：Public 规则读缓存）
```

| 进程 | 职责 |
| --- | --- |
| [auction-public-api](apps/auction-public-api/README.md) | 玩家读接口、竞价/代理/标记命令、登录会话、WebSocket 推送；内嵌 Public 页面 |
| [auction-admin-api](apps/auction-admin-api/README.md) | 管理接口与 Admin 页面；runtime 配置文件的唯一写者；分体部署的首次设置 |
| [auction-worker](apps/auction-worker/README.md) | 所有后台循环与外部集成；本机 control socket |
| [auction-server](apps/auction-server/README.md) | 可选：单一可执行文件，以子进程监管上述三者 |
| [auction-cli](apps/auction-cli/README.md) | 运维：配置检查、分体部署编排、迁移、备份恢复、业务运维 |

进程之间不直接调用业务接口：它们通过数据库共享事实。唯一的进程间通道是 Worker control（Unix socket，peer 凭据授权），用于就绪探测、状态读取与手动触发邮件轮询。

## 分层

```text
apps/*（装配根）
   │  读取配置、连接数据库、选择适配器、启动运行时
   ▼
适配器：auction-http · auction-storage · auction-integrations · auction-jobs · auction-cache · auction-setup
   │  实现端口
   ▼
auction-application（端口 + 用例）
   ▼
auction-domain（纯领域规则）
```

| crate | 层 | 说明 |
| --- | --- | --- |
| [auction-domain](crates/auction-domain/README.md) | 领域 | 拍卖状态机、竞价/代理规划、结算计算、义务与资金规则、装备词典；无 I/O |
| [auction-application](crates/auction-application/README.md) | 应用 | 端口（trait）、命令、安全投影与用例编排；不依赖具体数据库或网络实现 |
| [auction-storage](crates/auction-storage/README.md) | 适配器 | PostgreSQL 实现、迁移执行、运维数据库能力 |
| [auction-integrations](crates/auction-integrations/README.md) | 适配器 | MoogleMail 与 E-Hentai 客户端、论坛渲染/解析、装备来源、加密信封 |
| [auction-http](crates/auction-http/README.md) | 适配器 | 两个 HTTP 表面、安全边界、会话、WebSocket Hub、内嵌资源 |
| [auction-jobs](crates/auction-jobs/README.md) | 适配器 | Worker 调度运行时与 control 服务端/客户端 |
| [auction-cache](crates/auction-cache/README.md) | 适配器 | 可丢弃的读模型缓存（Redis 可选） |
| [auction-setup](crates/auction-setup/README.md) | 适配器 | 首次部署设置服务 |
| [auction-config](crates/auction-config/README.md) | 基础设施 | TOML 配置加载、合并与校验，runtime 文件存储 |
| [auction-observability](crates/auction-observability/README.md) | 基础设施 | 日志、指标、tracing、心跳 |
| [auction-process](crates/auction-process/README.md) | 基础设施 | 信号订阅、有界排空、任务守卫 |
| [auction-contracts](crates/auction-contracts/README.md) | 契约 | 前后端共用的 HTTP/WebSocket DTO（可编译到 wasm32） |
| [auction-web-shared](crates/auction-web-shared/README.md) | 前端 | 两个前端共用的浏览器机制、组件与设计 token |
| [auction-web-public](crates/auction-web-public/README.md) / [auction-web-admin](crates/auction-web-admin/README.md) | 前端 | 公开站点 / 管理后台（Dioxus 0.7 CSR） |

依赖规则：

- 依赖只向下：适配器依赖 application/domain，application 只依赖 domain；domain 不依赖任何内部 crate。
- 前端只依赖 `auction-contracts` 与 `auction-web-shared`，不依赖任何后端 crate。
- Public 二进制不包含 Admin 前端，Admin 不包含 Public 前端，Worker 不包含任何前端；`auction-server` 不依赖 `auction-cli`。发布脚本用 `cargo tree` 检查这些边界。
- 能力缺失（未配置数据库、密钥或外部凭据）时端口为空，接口明确返回“能力不可用”，不伪造成功或空数据。

## 一致性模型

### 命令与幂等

所有改变状态的操作都是带 `command_id` 的命令。用例在**一个 Serializable 事务**内完成：查重命令账本 → 锁定并读取快照 → 调用领域规划 → 写入事实、命令结果、审计与 realtime 事件。序列化冲突与死锁有界重试；同一 ID 同一请求重放原结果，不同请求返回冲突。客户端在结果不确定时以同一 ID 重试即可安全收敛。

### 追加式事实

出价、代理转换、补偿流水、黑名单历史、审计、发送尝试证明等只追加、不修改（数据库触发器强制）。当前状态（lot 当前价、领先者等）是这些事实的投影，由同一事务维护。

### Worker 的工作项

Worker 的每类工作都存放在数据库中：领取时写入租约与递增的 fencing 版本，执行中续租，完成时用 fencing 条件写回。进程崩溃后租约过期即可被重新领取。涉及外部写入（发邮件、发帖、发私信）的步骤先持久化“发送中”与请求摘要，再调用远端；远端结果不确定时转为**人工对账**，绝不自动重发——宁可停下，也不重复寄出 Credits 或装备。

### 运行时独占

- 每个外部 Cookie（邮件、论坛）同一时刻只能被一个进程使用：Worker 以 PostgreSQL session advisory lock 取得运行时所有权，连接断开即失去，失去后进程排空并退出重启。
- 每个 Public 实例以自己的 `consumer_key` 独占 realtime 消费游标。

### 时间

领域规划不读取系统时钟，时间由 `Clock` 注入；排期按冻结的 IANA 时区与本地钟点换算，不从当前配置反推历史。

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

每个角色一个 TOML 角色文件，引用共用的 `common.toml`（日志、指标、缓存、拍卖策略）、敏感文件 `secrets.toml`（数据库、Cookie、密钥，仅 owner 可读）与可选的 runtime 文件 `runtime/admin.toml`（由 Admin 后台编辑、Worker 读取被接管的分组）。所有配置错误在启动前失败。详见[配置参考](ops/config/README.md)。

## 安全边界

- Public 与 Admin 分别监听，建议只在 loopback 或 Unix socket 上监听并由反向代理终止 TLS；Admin 不应暴露到公网。
- Cookie 均为 HttpOnly；Admin 写请求要求 CSRF token；状态变更请求校验 Origin allowlist；登录、写请求与实时连接按客户端地址限流。
- 敏感数据静态加密：外部 Cookie、装备 key、用户登录 key 各自用独立的数据密钥（信封加密）保存；日志、指标与错误响应只包含安全分类与标识，不包含秘密或邮件正文。
- 浏览器 CSP 只允许同源脚本与 `wasm-unsafe-eval`，不加载第三方资源。

## 部署形态

| 形态 | 说明 |
| --- | --- |
| 分体部署 | 三个 systemd unit，由 `auction-cli service`（或 `auction-stack.service`）按 Worker → Admin → Public 顺序编排；适合需要独立重启或分机部署的场景 |
| 合并部署 | 一个 `auction-server` unit，以子进程运行三个角色；首次启动提供本机设置页并初始化空库 |

两种形态使用相同的配置目录布局与同一个 `auction-cli`。详见[运维手册](ops/README.md)。
