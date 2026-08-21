# 从 Auction-BF 迁移到 Rust

当前仓库是空的 Rust 起点，不会在旧代码上做语言级逐文件翻译。兄弟目录
`../Auction-BF` 中的旧仓库作为行为基准，迁移顺序如下：

1. **契约冻结：** 整理 Public/Admin route、JSON 字段、状态码、WebSocket/SSE 事件、
   TOML 配置字段和 migration 序号。新 Rust 配置明确采用“普通主配置 + 敏感 TOML”，不为
   旧单文件中的数据库连接串、Cookie 或会话密钥保留兼容读取。
2. **领域先行：** 用 Rust 重写状态机、竞价、代理竞价、软延时、结算和幂等规则；先
   记录旧系统的边界案例，单元测试按当前要求暂缓。
3. **持久化：** 先设计 PostgreSQL 完整生产 schema、repository、锁、幂等、审计和 outbox；
   再按 Lite 能力清单建立 SQLite 所需的 schema 子集与串行事务适配器，不要求迁移或系统
   能力一一对等。测试恢复后再补各档位自己的事务验证。
4. **Application ports：** 接入 Users、Rules、Delivery、Audit、Realtime 等 owner，
   明确跨模块只允许窄端口。缓存只接入可重建读取投影，默认关闭；Redis 不得成为竞价、
   幂等、会话或审计的权威状态。
5. **HTTP surface：** 先实现 Public，再实现 Admin；保持旧 route 和错误契约，使用
   contract tests 防止迁移期间漂移。
6. **Worker/integrations：** 接入 E-Hentai 发帖/回帖扫描、MoogleMail、通知、重试和
   durable cursor；每个外部写入都要有幂等键和恢复策略。
7. **Dioxus UI：** 先做只读拍卖列表和详情，再接入登录、出价、代理和管理命令；Public 与
   Admin 各自独立的组件树与路由树按旧 `manifests/*.routes.json` 的 URL 契约推进，realtime
   使用 WebSocket。浏览器行为验证在用户明确要求后才纳入范围。
8. **切换：** 双跑只读流量，比较事件/余额/竞价结果；确认数据和运维 runbook 后再切写。

在第 1 步完成前，不应把旧数据库表直接当作 Rust domain model；旧 schema 的兼容读取
与新 schema 的写入需要单独的 anti-corruption adapter。
