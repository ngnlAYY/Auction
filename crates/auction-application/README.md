# auction-application

应用层：用例、窄端口和事务编排。

源码布局：`src/lib.rs` 只保留模块声明和公共导出；实现按端口、服务和错误职责放在对应子目录。
`ports/` 与 `services/` 再按 `admin/`、`auction/`、`mail/`、`realtime/`、`recovery/`、
`settlement/` 等 owner 分组，避免 `admin_*`、`recovery_*` 一类文件在同层堆叠。

- `src/ports/core/{policy,ledger,repository,competition,clock}.rs`：事务策略、命令幂等账本、
  拍卖仓储、竞价事务和 UTC 时钟核心端口；`src/ports/mod.rs` 只保留模块装配与稳定公共导出。
  使用 Rust 原生 async trait 返回类型，不依赖 `async-trait` 宏。
- `src/services/auction/commands.rs`：拍卖创建用例，例如 `AuctionService::create`。
- `src/services/auction/lifecycle/`：Admin/Worker 共享的拍卖生命周期用例；`mod.rs` 持有服务状态、
  命令幂等事务编排和端口实现，`plan.rs` 持有状态迁移与 final proxy 计划，`schedule.rs` 持有后继轮次、
  安全日期计算和稳定 schedule ID；
  人工提前开放与提前结算使用启动时冻结且默认关闭的策略，并在事务锁内失败关闭；已到期的计划开放和
  Worker due 命令不受人工提前开放开关影响。人工 Final 把较晚截止时间收紧到冻结的最终窗口并保留
  软截止竞价；提前结算只把截止时间收紧到当前事务时间，让既有 Worker 继续拥有结算事实。生命周期端口会把锁前上下文、动作计划和后端实际写入摘要
  一并交给同事务审计边界，使 Preview/Open/Cancel/Final 与新增 EarlySettlement 的 action metadata 可精确恢复，
  又不把 PostgreSQL/SQLite 行模型泄漏到应用层。
- `src/ports/auction/slow_proxy.rs`、`src/services/auction/competition.rs` 与
  `src/services/auction/competition/legacy_ledger.rs`：Public 竞价及 Worker slow due 命令端口和用例；
  父服务使用数据库时间重读 generation/授权/资格 fence，并复用同一竞价事务，不在 Worker 中复制
  状态机。旧版稳定命令 ID 与请求摘要由端口值对象统一派生；账本防腐层逐字复现 Auction-BF 的
  `OK + BidResult/ProxyResult`、确定性事实/授权 ID 和严格重放校验，同时只读兼容已存在的新版
  `accepted + CompetitionPlan` 记录；无公开事实的旧代理结果按 Go nil slice 接受并输出
  `facts:null`，但字段缺失、未知字段和非空错误事实仍失败关闭。外部 Public 普通竞价、代理创建和代理降低还通过
  `CompetitionCommandAuditContext` 把命令级审计身份带到同一事务，前置资格冻结只追加代理转换事实，
  不重复占用 Admin 审计记录。命令端口返回独立的 typed outcome：领域计划与代理成功响应所需的
  授权、quota、完整转换历史和终态原因在同一事务内冻结，HTTP 不需要在提交后读取账户投影；历史
  `accepted` 行只在同一只读事务按授权版本截取转换历史并读取冻结策略 quota，不重新执行命令，也不以
  当前生命周期或时钟 gate 否定既有成功事实。
- `src/services/settlement/commands/` 与 `src/ports/settlement/commands.rs`：结算命令在锁定拍卖前先读取
  命令账本，以保持旧版冲突优先级；当前格式重放时还会从冻结策略、首次结算资金池和锁内 lot
  事实重算完整计划及义务，只有持久化摘要与 payload 全量一致才返回。接管的旧
  `settlement_complete/OK` 则逐字复现 Go 请求摘要、严格解析完整结果，并与已结算摘要、lot 金额和
  义务种类数量逐项对账。旧格式只返回从权威事实重建的只读计划，不重新执行结算，也不改写历史账本。
  `LegacySettlementVerificationPort` 另为 `auctionctl` 提供脱敏只读核验摘要；缺账本时没有写入回退，
  存储适配器必须在同一一致快照中提供旧命令身份和权威结算事实。
- `src/services/settlement/fulfillment/`：结算履约应用服务；`service.rs` 编排 prepare/activate/reconcile，
  `intent.rs` 负责无 secret delivery intent，`validation.rs` 与 `evidence.rs` 负责 claim/外部回执
  校验，`proof.rs` 负责稳定摘要，`model.rs` 持有跨 owner 载荷模型；schema v1 intent/payload 使用
  与旧版 Go 相同的 canonical JSON、稳定 ID 和摘要，装备 secret 不进入持久化 payload。
  `WinnerPaymentCarrierCod` 的义务/装备来源 world 保持 `isekai`，delivery world 固定为 `main`；
  `mod.rs` 只负责模块装配和稳定出口。
- `src/services/recovery/commands.rs`：Admin/Worker 共用的 recovery 确认入口校验与结果 fence。
- `src/services/admin/rules.rs`：规则文档的候选校验、revision CAS、命令重放和发布事务编排。
- `src/ports/admin/runtime_config/`：Admin runtime 的非敏感文档模型、命令账本模型、适配器端口、
  错误类型和本地边界校验；`business.rs` 冻结旧版 main/isekai、发件人 allowlist、装备过滤和 14 个
  通知模板的闭集，供 Admin 编辑并由 Worker 启动时投影；`runtime_config.rs` 只负责稳定公共类型出口，
  不执行文件、数据库或网络操作。
- `src/ports/admin/credentials/`：敏感命令/写入载荷位于 `commands.rs`，查询、加密、Worker runtime 和
  dashboard 端口位于 `ports.rs`；`value/{model,secret,runtime,health,error,common}.rs` 分别负责闭集模型、
  明文/密文安全值、运行记录、dashboard 聚合、业务错误和共用边界；`credentials.rs` 只负责稳定公共类型出口，
  明文和密文的安全边界保持不变。
- `src/ports/admin/users/`：安全用户投影、列表请求和分页位于 `model.rs`，用户/恢复命令、写入
  载荷和安全结果位于 `commands.rs`，密钥、Users 存储和 recovery 黑名单窄端口位于 `ports.rs`，
  共享字段校验位于 `common.rs`；`users/mod.rs` 只负责稳定公共类型出口。
- `src/services/admin/users/`：`query.rs` 负责有界用户列表，`commands.rs` 负责创建/修改/改名回滚
  事务编排，`digest.rs` 负责幂等请求摘要，`validation.rs` 负责动作字段校验；`mod.rs` 只保留
  服务状态、构造函数和 `AdminUserPort` 适配。
- `src/services/admin/credentials/`：`query.rs` 负责有界凭据安全投影，`commands.rs` 负责导入/改名/启停/删除
  事务编排与安全结果重放，`digest.rs` 负责不包含明文 secret 的幂等请求摘要，`validation.rs` 负责命令字段校验；
  `mod.rs` 只保留服务状态、构造函数和 `AdminCredentialPort` 适配。
- `src/ports/public/cache.rs` 与 `src/services/public/rules_cache.rs`：可重建读模型缓存端口和
  Public 规则快照装饰器；只缓存完整有效文档，miss、损坏、TTL 到期或后端失败都回源权威规则表。
- `src/ports/admin/intake/` 与 `src/services/admin/intake.rs`：`equipment.rs` 负责 URL/canonical JSON
  装备事实和 key 清理，`commands.rs` 负责命令摘要，`source.rs` 负责来源快照，`records.rs` 负责
  脱敏录入写入计划/结果，`ports.rs` 负责 key 信封与事务端口，`common.rs` 负责错误、字段校验和
  启动时冻结的可交易限制策略；`mod.rs` 只负责稳定导出，明文 key 不进入写端口。默认策略允许
  Soulbound 装备，只有 Admin runtime 显式开启时才在事务前拒绝；邮件过滤使用自己的严格策略。
- `src/ports/auction/policy.rs`：拍卖创建时冻结的经济、结算和恢复策略模板；配置适配器只
  转换为该类型，应用服务和 PostgreSQL 创建事务共享同一份策略来源。
- `src/ports/auction/queries/`：Public 世界/拍卖目录、lot/装备投影、账户代理、竞价日志和
  成交价格历史分别由 `world.rs`、`auction.rs`、`lot.rs`、`account.rs`、`bid_log.rs` 和
  `price_history.rs` 持有；`auction.rs` 还提供单拍卖 WebSocket 升级前的公开状态闭集，避免 HTTP
  从事件历史猜测 settled 状态；`queries.rs` 只负责稳定公共导出，不把数据库记录暴露给表面层。
- `src/ports/admin/forum.rs`：Admin 论坛 HTML/文本预览的有界输入、catalog/posts/warnings
  安全结果和解析端口；该端口只表达本地预览，不承担远程发布或数据库写入。
- `src/ports/admin/audit.rs`：Admin 审计精确筛选、时间范围、keyset/数字分页和安全 metadata
  投影端口；只读结果不暴露数据库行。
- `src/ports/admin/mail/` 与 `src/services/admin/mail.rs`：Admin 邮件轮询状态、入站审查
  安全投影、状态版本 fence、暂停/恢复及历史兼容命令端口和命令账本；Admin A20 的全局
  唤醒结果由 Worker control 端口负责，不承载 MoogleMail 远程传输。
- `src/ports/admin/mail/`：轮询状态和入站安全投影位于 `model.rs`，暂停/触发命令和账本记录位于
  `commands.rs`，应用/事务错误位于 `errors.rs`，查询、命令和事务窄端口位于 `ports.rs`，
  常量、聚合标识和字段校验位于 `common.rs`；`mail/mod.rs` 只负责稳定公共导出。
- `src/ports/mail/poll.rs`：Worker 按 mailbox_key 领取邮箱的 claim、lease/fencing、解析后安全入站事实、cursor、
  失败退避和同事务幂等提交端口；不保存原始 HTML、Cookie、token、正文或装备 key。
- `src/ports/mail/processing/`：装备过滤、短生命周期邮件 envelope、拍卖入站、delivery outbox 和业务决定
  分别位于 `filter.rs`、`message.rs`、`auction.rs`、`delivery.rs`、`decision.rs`；`processing.rs`
  只负责稳定公共导出，并由 `common.rs` 统一持有安全错误和敏感值清理辅助。
- `src/errors/mod.rs` 的 `MailProcessingRetryability` 把应用错误冻结为 `Retryable`/`Permanent`；Worker
  只可对前者和超时执行有界 inline retry，确定性输入、配置和业务拒绝必须立即失败关闭。
- `src/ports/mail/user/`：`model.rs` 负责注册/换绑结果闭集，`security.rs` 负责短生命周期登录 key
  与密文信封，`commands.rs` 负责 Users owner 写入载荷，`ports.rs` 负责存储、签发、读取和用例窄端口；
  `user/mod.rs` 只负责稳定公共导出。
- `src/services/mail/processing.rs` 只负责 MoogleMail 应用服务的模块出口；`processing/intake.rs`、
  `processing/commands.rs`、`processing/pricing.rs` 分别持有拍卖入站、命令幂等辅助和起拍价/装备身份解析；
  `processing/processor/{routing,auction,delivery}.rs` 分别持有主题路由/用户命令、装备入站和无密文
  delivery outbox 编排，`processor.rs` 只保留配置、处理器状态和 `MailProcessor` 公共出口。
- `src/ports/operations/alert.rs` 只负责 operations alert 端口出口；`operations/alert/` 下按
  signals、model、evaluate、delivery、health、digest 拆分领域投影、阈值评估、批次状态和摘要哈希，
  不把告警结果混入拍卖或外部传输事实。`delivery_returned_mail` 是已证明收件人不存在且需要取回
  资产的人工对账子信号；它保留独立 fingerprint，同时不替代 delivery 人工对账总水位。
- `src/ports/scheduling/recurring.rs`：Worker 读取下一次 UTC 触发时刻的窄端口；cron、时区和
  固定间隔解析留在配置适配器，不能替代持久化 due、游标或租约。
- `src/errors/mod.rs`：应用错误，保留领域错误分类。

Public 规则文档和黑名单只读契约分别位于 `src/ports/public/rules.rs` 与
`src/ports/public/blacklist.rs`；它们只定义安全投影、语言/分页边界和 owner 端口，不包含 SQL。
Public 补偿池与账本只读契约位于 `src/ports/public/compensation/`，由批次摘要/分页与逐笔
movement 校验分别拥有，定义守恒摘要、闭集 movement、编号页和 keyset cursor 边界，不依赖
PostgreSQL 或 SQLite 类型。
Public 论坛镜像只读契约位于 `src/ports/forum/public.rs`，只表达本地同步状态和已发布内容，不承担
远程论坛传输。
论坛远端同步恢复契约位于 `src/ports/forum/sync.rs`：`ForumSyncRecoveryPort` 只允许按精确标题读取
远端 topic，并把未知写入分成“只读对账”和“等待持久人工授权”两个阶段；端口本身不决定 CLI
确认文字、数据库表或 HTTP 传输实现。
Admin lot 列表只读契约位于 `src/ports/admin/lots.rs`，定义拍卖/lot/装备/投稿的安全快照和
有界分页；它要求完整 Admin 生命周期投影，不把 Lite 缺失字段填成默认成功值。
Admin draft lot 修改契约位于 `src/ports/admin/lot_commands.rs`，只允许版本 fence 保护的
撤回与起拍价动作；服务层统一编排命令账本、Serializable 事务、预览 revision、审计和
withdrawn-return 履约义务，具体 SQL 不跨过端口。
Admin 拍卖列表只读契约位于 `src/ports/admin/auctions.rs`，定义 world/status 筛选、旧页码
分页、生命周期安全快照和 lot 数量；SQLite Lite 缺少完整投影时必须显式报告能力不支持。
Admin 规则文档契约位于 `src/ports/admin/rules.rs`，区分候选校验、读取快照和 PostgreSQL
完整档发布事务；发布使用 `command_id`、request hash 与 revision fence，SQLite 只保留读取。
Admin 装备录入契约位于 `src/ports/admin/intake/`，区分外部来源读取、key 加密和数据库
写入；来源失败或可选的可交易策略拒绝不会打开数据库写事务，重复命令只重放已持久化的安全结果。

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
