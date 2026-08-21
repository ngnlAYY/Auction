# 从 Auction-BF 迁移到 Rust

当前仓库是空的 Rust 起点，不会在旧代码上做语言级逐文件翻译。兄弟目录
`../Auction-BF` 中的旧仓库作为行为基准，迁移顺序如下：

1. **契约冻结：** 整理 Public/Admin route、JSON 字段、状态码、WebSocket/SSE 事件、
   TOML 配置字段和 migration 序号。
2. **领域先行：** 用 Rust 重写状态机、竞价、代理竞价、软延时、结算和幂等规则；先
   记录旧系统的边界案例，单元测试按当前要求暂缓。
3. **持久化：** 设计 PostgreSQL/SQLite 的同逻辑 schema，分别建立追加式 migration；
   实现 repository，测试恢复后再补 backend-specific 事务验证。
4. **Application ports：** 接入 Users、Rules、Delivery、Audit、Realtime 等 owner，
   明确跨模块只允许窄端口。
5. **HTTP surface：** 先实现 Public，再实现 Admin；保持旧 route 和错误契约，使用
   contract tests 防止迁移期间漂移。
6. **Worker/integrations：** 接入 E-Hentai 发帖/回帖扫描、MoogleMail、通知、重试和
   durable cursor；每个外部写入都要有幂等键和恢复策略。
7. **Leptos UI：** 先做只读拍卖列表和详情，再接入登录、出价、代理和管理命令；SSR、
   hydration 和 realtime 通过浏览器测试验证。
8. **切换：** 双跑只读流量，比较事件/余额/竞价结果；确认数据和运维 runbook 后再切写。

在第 1 步完成前，不应把旧数据库表直接当作 Rust domain model；旧 schema 的兼容读取
与新 schema 的写入需要单独的 anti-corruption adapter。
