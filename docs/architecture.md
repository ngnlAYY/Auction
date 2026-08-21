# Rust 重构架构

## 总体原则

Rust 版本从零开始，不把 Go 的包名机械翻译成 Rust module。边界按“谁拥有业务事实”
划分，依赖只向内：

```text
apps (composition roots)
  ├── HTTP / Worker / CLI adapters
  └── infrastructure adapters
          ↓
      application (use cases + ports)
          ↓
       domain (invariants, state machines, value objects)
```

`auction-domain` 不依赖 Axum、Dioxus、SQLx、reqwest 或环境变量。`auction-application`
只声明端口；数据库、论坛、邮件和 HTTP 都在外层实现端口。

“全 Rust”是仓库代码和程序库依赖约束，不是基础设施采购约束。PostgreSQL、SQLite、
Redis、反向代理和系统服务可使用非 Rust 官方实现，并通过适配器边界接入。

## Crate 所有权

| crate | 所有内容 | 明确不负责 |
| --- | --- | --- |
| `auction-domain` | Auction/Lot/Bid/Proxy/Settlement 模型、状态机、不变量 | HTTP、SQL、日志、外部网络 |
| `auction-application` | Create/Open/PlaceBid/Proxy/Settle 等用例、事务端口、幂等策略 | 具体数据库和路由 |
| `auction-contracts` | REST/WebSocket DTO、错误编码、分页和版本契约 | 领域写入和 SQL 模型 |
| `auction-storage` | PostgreSQL 完整适配器、SQLite Lite 适配器、migration、query、repository | 竞价规则本身 |
| `auction-cache` | 默认关闭的缓存边界、Redis 连接和带 TTL 的可重建值 | 权威状态、锁、幂等和审计 |
| `auction-integrations` | E-Hentai、MoogleMail、身份和通知适配器 | 拍卖状态迁移 |
| `auction-jobs` | due task、论坛扫描、投递重试、生命周期调度 | HTTP handler |
| `auction-http` | Public/Admin router、认证 middleware、DTO 映射、前端产物内嵌 | 领域决策 |
| `auction-web-shared` | 设计变量、基础排版、语言集合与偏好持久化、无业务偏向的展示组件 | 任一 surface 的业务组件与文案 |
| `auction-web-public` | 公开端 Dioxus 组件、路由、公开端译文 | 数据库、secret、管理端能力 |
| `auction-web-admin` | 管理端 Dioxus 组件、路由、管理端译文 | 数据库、secret、公开端组件树 |
| `auction-config` | 配置解析、默认值、校验 | 业务规则 |
| `auction-observability` | tracing、metrics、health、correlation id | 业务日志内容 |

浏览器端是纯 CSR SPA。`dx build --platform web` 为每个 surface 生成静态
WASM/JS/HTML，`auction-http` 用 `rust-embed` 分别内嵌，产出单文件制品。Public 与 Admin
的组件树、路由树和静态产物完全隔离：公开端二进制不包含管理端资源。产物缺失时静态路由
返回 503 并说明需要执行的构建命令，不影响 `/health` 契约，也不让进程崩溃。

界面文案由 `dioxus-i18n` 从各 surface 的 `i18n/*.ftl` 取用，支持 `en`/`zh-CN`/`zh-TW`/
`ja`/`ko`。公开端默认英文，管理端默认简体中文。译文资源按 surface 隔离，语言选择能力
（受支持集合、`hv.lang` 持久化、`<html lang>` 同步）由 `auction-web-shared` 统一提供。

## 运行进程

```text
auction-public-api  ─┐
auction-admin-api   ─┼─ auction-application
auction-worker      ─┘  ├─ auction-storage ─┬─ PostgreSQL（完整生产档）
                       │                   └─ SQLite（Lite 档）
                       ├─ auction-integrations ─ E-Hentai/MoogleMail
                       ├─ auction-jobs
                       └─ auction-cache ─ Redis（可选）
auctionctl ──────────────────────────────── ops/migrations/config
```

Public 和 Admin 可以分别绑定端口、Unix socket 或反向代理；Worker 不暴露业务写入
HTTP；所有写入仍必须经过 application owner 的命令端口。这样保留 Auction-BF 的部署
边界，同时把组合根压缩到 `apps/*/src/main.rs`。

## 部署档位

| 档位 | 数据库与拓扑 | 能力边界 |
| --- | --- | --- |
| 完整生产档 | PostgreSQL；Public、Admin、Worker 可独立部署并扩展 | 完整持久化能力、并发写入、多进程和后续高可用能力 |
| Lite 档 | SQLite；单主机、每个角色至多一个实例、Worker 并发为一 | 本地、演示和低并发部署；不承诺多副本、故障转移、在线迁移或全部生产功能 |

`database_url` 的 scheme 决定档位，不额外引入会与数据库选择冲突的 profile 开关。Lite
档可以缺少 PostgreSQL 专属或高并发系统能力，但每项缺失都必须在对应模块和部署文档中
明确；已经声明支持的竞价命令仍必须遵守领域不变量、事务原子性、幂等和追加式审计。
SQLite 数据库文件只能位于单主机受支持的本地文件系统，不能把网络共享文件系统当作
横向扩展方案。

## 缓存边界

缓存默认关闭。需要 Redis 时，对目标进程启用 `cache-redis` Cargo feature，在主配置中把
`cache.backend` 设为 `redis`，并仅在敏感 TOML 中提供 `redis_url`。实现直接复用 Redis
客户端的异步连接管理，不额外引入通用连接池或进程内缓存库。

缓存只能保存可重建的查询结果或派生视图，并必须带有限 TTL。读取 miss、Redis 不可用和
失效失败都回退到权威数据源；竞价事务、资格判断、命令幂等、锁、会话真值和审计不能依赖
缓存。由此，未编译 Redis feature、运行时选择 `disabled` 或 Redis 临时故障都不改变业务
正确性。

## 配置与敏感信息

每个主配置只包含监听地址、日志过滤器、缓存策略等普通字段，并通过 `secrets_file` 引用
第二个 TOML。`database_url`、`redis_url`、会话签名密钥和外部服务 Cookie 只存在于敏感
文件；不兼容旧的单文件字段，也不提供环境变量覆盖。相对敏感文件路径以主配置所在目录
解析，部署时该文件必须排除版本控制并限制为运行账号可读。

### Lite 能力边界

| 能力 | PostgreSQL 完整档 | SQLite Lite 档 |
| --- | --- | --- |
| 已实现命令的数据正确性 | 完整领域不变量 | 同一不变量，不允许弱化 |
| schema 与 migration | 完整生产 schema，追加式迁移 | 仅功能子集，追加式迁移，可接受维护窗口 |
| 写入并发 | 行锁、多个并发事务 | 单连接串行写入、低并发 |
| 进程拓扑 | 独立进程与多副本 | 单主机、每个角色一个实例 |
| Worker | 持久化游标、可扩展并发 | 若启用则游标仍持久化，但实例和并发均为一 |
| 高可用与扩展 | 可建设故障转移、只读副本和横向扩展 | 不支持多副本、故障转移或网络共享数据库 |

当前仓库仍是启动骨架，这张表定义的是实现上限和验收边界，不表示对应业务功能已经完成。
以后新增持久化能力时，必须先标明它属于“完整档”“Lite 档”或“两者”，再决定是否需要
SQLite migration 和 repository 实现。

## 竞价写入不变量

第一版实现必须以以下顺序落地，不能先做页面再补一致性。PostgreSQL 实现完整用例和
repository port；SQLite 只实现 Lite 能力需要的端口，但已实现端口的事务细节和不变量
不能降级：

1. 为每个命令生成稳定 `command_id`，同一命令重放返回同一结果。
2. 在一个数据库 transaction 中锁定 auction/lot 和活动代理授权；PostgreSQL 使用行锁，
   SQLite 使用明确的串行写事务，不能假设存在 `FOR UPDATE`。
3. 检查竞价资格、状态、金额、代理上限和有效结束时间。
4. 写入 bid fact、状态版本、audit event、outbox/event record。
5. 事务提交后由 Worker 投递通知；外部失败不能回滚已经接受的竞价事实。

PostgreSQL 是完整生产后端。SQLite 只是非对等 Lite 替代品，适用于本地开发、演示和
低并发单节点部署；必须在 runbook 中说明单写者、锁等待、备份和文件系统假设。

## 参考 Honk 的部分

Honk 使用根 Cargo workspace 管理多个独立 crate，并把设计文档、CI 和运维文件作为
一等目录；本项目采用相同的工程形态，但按 Auction 的 domain/application/adapters
边界重新命名。参考：<https://github.com/daeuniverse/honk>。
