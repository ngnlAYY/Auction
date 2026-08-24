# auction-application

应用层：用例、窄端口和事务编排。

源码布局：`src/lib.rs` 只保留模块声明和公共导出；实现按端口、服务和错误职责放在对应子目录。
`ports/` 与 `services/` 再按 `admin/`、`auction/`、`mail/`、`realtime/`、`recovery/`、
`settlement/` 等 owner 分组，避免 `admin_*`、`recovery_*` 一类文件在同层堆叠。

- `src/ports/mod.rs`：`AuctionRepository`、`CompetitionTransaction`、`TransactionRunner`、
  `Clock`、`CommandLedger`、recovery 报告/确认和
  `TransactionPolicy` 等端口。使用 Rust 原生 async trait 返回类型，不依赖 `async-trait` 宏。
- `src/services/auction/commands.rs`：拍卖创建用例，例如 `AuctionService::create`。
- `src/services/auction/lifecycle.rs`：Admin/Worker 共享的拍卖状态迁移、命令账本和事务编排；Admin
  人工提前开放使用启动时冻结的策略，并在事务锁内读取计划开放时间后失败关闭，已到期的计划开放和
  Worker due 命令不受该人工授权开关影响。
- `src/services/auction/competition.rs`：竞价用例，例如 `CompetitionService::place_bid`。
- `src/services/recovery/commands.rs`：Admin/Worker 共用的 recovery 确认入口校验与结果 fence。
- `src/services/admin/rules.rs`：规则文档的候选校验、revision CAS、命令重放和发布事务编排。
- `src/ports/public/cache.rs` 与 `src/services/public/rules_cache.rs`：可重建读模型缓存端口和
  Public 规则快照装饰器；只缓存完整有效文档，miss、损坏、TTL 到期或后端失败都回源权威规则表。
- `src/ports/admin/intake.rs` 与 `src/services/admin/intake.rs`：URL/canonical JSON 装备来源、
  脱敏录入写入计划、key 信封端口、命令摘要和 PostgreSQL 事务编排；明文 key 不进入写端口。
- `src/ports/auction/policy.rs`：拍卖创建时冻结的经济、结算和恢复策略模板；配置适配器只
  转换为该类型，应用服务和 PostgreSQL 创建事务共享同一份策略来源。
- `src/ports/admin/forum.rs`：Admin 论坛 HTML/文本预览的有界输入、catalog/posts/warnings
  安全结果和解析端口；该端口只表达本地预览，不承担远程发布或数据库写入。
- `src/ports/admin/audit.rs`：Admin 审计精确筛选、时间范围、keyset/数字分页和安全 metadata
  投影端口；只读结果不暴露数据库行。
- `src/ports/admin/mail.rs` 与 `src/services/admin/mail.rs`：Admin 邮件轮询状态、入站审查
  安全投影、状态版本 fence、暂停/恢复及历史兼容命令端口和命令账本；Admin A20 的全局
  唤醒结果由 Worker control 端口负责，不承载 MoogleMail 远程传输。
- `src/ports/mail/poll.rs`：Worker 按 mailbox_key 领取邮箱的 claim、lease/fencing、解析后安全入站事实、cursor、
  失败退避和同事务幂等提交端口；不保存原始 HTML、Cookie、token、正文或装备 key。
- `src/ports/scheduling/recurring.rs`：Worker 读取下一次 UTC 触发时刻的窄端口；cron、时区和
  固定间隔解析留在配置适配器，不能替代持久化 due、游标或租约。
- `src/errors/mod.rs`：应用错误，保留领域错误分类。

Public 规则文档和黑名单只读契约分别位于 `src/ports/public/rules.rs` 与
`src/ports/public/blacklist.rs`；它们只定义安全投影、语言/分页边界和 owner 端口，不包含 SQL。
Public 补偿池与账本只读契约位于 `src/ports/public/compensation.rs`，定义守恒摘要、闭集 movement、
编号页和 keyset cursor 边界，不依赖 PostgreSQL 或 SQLite 类型。
Public 论坛镜像只读契约位于 `src/ports/forum/public.rs`，只表达本地同步状态和已发布内容，不承担
远程论坛传输。
Admin lot 列表只读契约位于 `src/ports/admin/lots.rs`，定义拍卖/lot/装备/投稿的安全快照和
有界分页；它要求完整 Admin 生命周期投影，不把 Lite 缺失字段填成默认成功值。
Admin draft lot 修改契约位于 `src/ports/admin/lot_commands.rs`，只允许版本 fence 保护的
撤回与起拍价动作；服务层统一编排命令账本、Serializable 事务、预览 revision、审计和
withdrawn-return 履约义务，具体 SQL 不跨过端口。
Admin 拍卖列表只读契约位于 `src/ports/admin/auctions.rs`，定义 world/status 筛选、旧页码
分页、生命周期安全快照和 lot 数量；SQLite Lite 缺少完整投影时必须显式报告能力不支持。
Admin 规则文档契约位于 `src/ports/admin/rules.rs`，区分候选校验、读取快照和 PostgreSQL
完整档发布事务；发布使用 `command_id`、request hash 与 revision fence，SQLite 只保留读取。
Admin 装备录入契约位于 `src/ports/admin/intake.rs`，区分外部来源读取、key 加密和数据库
写入；来源失败不会打开数据库写事务，重复命令只重放已持久化的安全结果。

Admin 拍卖创建会在 PostgreSQL Serializable 事务中把已校验的策略模板同时写入 schema v4
`policy_snapshot` 和当前仍被竞价/生命周期读取的遗留规则列；HTTP 创建命令不能注入策略字段。

依赖只指向 `auction-domain`。适配器在外层实现这里声明的端口。

**不得**读取进程配置、打开网络监听、直接执行 SQL 或直接调用外部服务。跨模块协作必须
通过窄端口，不共享大结构体绕过边界。

每个可变命令都要有稳定的 `command_id`，重复提交返回原结果；同一 ID 搭配不同负载必须报
冲突。竞价写入的事务顺序见 `docs/architecture.md` 的「竞价写入不变量」。

`TransactionPolicy::serializable()` 固定三次最大尝试，`read_committed()` 固定一次；
存储适配器不得把竞价写入和队列领取路径合并成同一种事务语义。命令账本的
`request_hash` 必须是固定 32 字节 SHA-256 摘要，结果负载由边界适配器保证为 JSON object。
