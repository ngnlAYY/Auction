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

`auction-domain` 不依赖 Axum、Leptos、SQLx、reqwest 或环境变量。`auction-application`
只声明端口；数据库、论坛、邮件和 HTTP 都在外层实现端口。

## Crate 所有权

| crate | 所有内容 | 明确不负责 |
| --- | --- | --- |
| `auction-domain` | Auction/Lot/Bid/Proxy/Settlement 模型、状态机、不变量 | HTTP、SQL、日志、外部网络 |
| `auction-application` | Create/Open/PlaceBid/Proxy/Settle 等用例、事务端口、幂等策略 | 具体数据库和路由 |
| `auction-contracts` | REST/WebSocket DTO、错误编码、分页和版本契约 | 领域写入和 SQL 模型 |
| `auction-storage` | PostgreSQL/SQLite pool、dialect migration、query、repository | 竞价规则本身 |
| `auction-integrations` | E-Hentai、MoogleMail、身份和通知适配器 | 拍卖状态迁移 |
| `auction-jobs` | due task、论坛扫描、投递重试、生命周期调度 | HTTP handler |
| `auction-http` | Public/Admin router、认证 middleware、DTO 映射 | 领域决策 |
| `auction-web` | Leptos SSR/hydrate 组件、surface 路由和交互 | 数据库和 secret |
| `auction-config` | 配置解析、默认值、校验 | 业务规则 |
| `auction-observability` | tracing、metrics、health、correlation id | 业务日志内容 |

## 运行进程

```text
auction-public-api  ─┐
auction-admin-api   ─┼─ auction-application ─ auction-storage ─┬─ PostgreSQL
auction-worker      ─┘             ├──────── auction-integrations ─ E-Hentai/MoogleMail
                                   └──────── auction-jobs
                                                               └─ SQLite
auctionctl ──────────────────────────────── ops/migrations/config
```

Public 和 Admin 可以分别绑定端口、Unix socket 或反向代理；Worker 不暴露业务写入
HTTP；所有写入仍必须经过 application owner 的命令端口。这样保留 Auction-BF 的部署
边界，同时把组合根压缩到 `apps/*/src/main.rs`。

## 竞价写入不变量

第一版实现必须以以下顺序落地，不能先做页面再补一致性。两种数据库共享用例和
repository port，但事务细节必须保留 backend-specific 实现：

1. 为每个命令生成稳定 `command_id`，同一命令重放返回同一结果。
2. 在一个数据库 transaction 中锁定 auction/lot 和活动代理授权；PostgreSQL 使用行锁，
   SQLite 使用明确的串行写事务，不能假设存在 `FOR UPDATE`。
3. 检查竞价资格、状态、金额、代理上限和有效结束时间。
4. 写入 bid fact、状态版本、audit event、outbox/event record。
5. 事务提交后由 Worker 投递通知；外部失败不能回滚已经接受的竞价事实。

PostgreSQL 是多进程生产部署的推荐后端。SQLite 适用于本地开发、快速测试和单节点
部署，必须在 runbook 中说明单写者和文件锁定假设。

## 参考 Honk 的部分

Honk 使用根 Cargo workspace 管理多个独立 crate，并把设计文档、CI 和运维文件作为
一等目录；本项目采用相同的工程形态，但按 Auction 的 domain/application/adapters
边界重新命名。参考：<https://github.com/daeuniverse/honk>。
