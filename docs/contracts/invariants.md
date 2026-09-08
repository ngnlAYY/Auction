# 领域不变量与写入语义

本文档定义 Auction Rust 必须满足的领域正确性约束和验收边界。这里描述的是实现契约，不是测试
用例清单；在用户明确授权前，验证范围仅为静态检查、源码审计和文档一致性。

## 命令、幂等和标识

### I-01 命令账本是权威幂等边界

每个可变命令都必须携带稳定的 command_id。数据库命令账本至少保存：

- command_id：UUID 主键；
- command_kind：非空命令类型；
- aggregate_id：受影响的聚合；
- request_hash：固定 32 字节请求摘要；
- result_code：稳定结果分类；
- result_payload：JSON 对象形式的结果；
- created_at：事务时间戳。

相同 command_id 和相同请求摘要必须重放原结果；相同 command_id 搭配不同摘要必须
返回冲突。重复请求不能再次写入竞价事实、审计记录、结算义务或外部命令。

### I-02 可重试写入使用确定性事实标识

重试、进程恢复和 Worker 重放不得根据随机 UUID 产生重复事实。竞价事实、代理授权
和其他可重试领域对象应使用由稳定业务输入派生的确定性标识，或由唯一约束和命令
账本共同保证只落一份。

## 事务、锁和原子写入

### I-03 竞价和生命周期写入不可拆分

竞价、代理出价、拍卖状态推进和影响结算的管理命令必须在同一数据库事务中完成：

1. 锁定受影响拍卖、标的和有效资格所需的权威行；
2. 校验命令幂等性、身份、资格、状态和版本；
3. 按领域规则计算新金额、结束时间和状态；
4. 写入领域事实、状态版本、审计事件及持久化事件/outbox；
5. 提交后才允许触发论坛、邮件、通知等外部写入。

禁止先在一个事务读取当前价，再在另一个事务写入竞价。事务失败或冲突时，整个
写入集合回滚，不留下半个竞价、半个审计事件或无法解释的 outbox 记录。

规则文档发布同样必须在一个 PostgreSQL Serializable 事务内完成：按 locale 做 revision
CAS，写入文档、`rules_admin_commands` 幂等结果和追加审计；同一 `command_id` 重放原文档，
摘要或 locale 不同则返回冲突。SQLite Lite 只提供规则读取，不把发布能力降级为无审计写入。

Admin 关闭业务世界同样是关键管理写入：必须先锁定 `worlds` 权威行，再检查该世界是否仍有任何
非 `settled`/`cancelled` 拍卖或仍可领取的活动未完成调度轮次，以 `policy_revision` 做 CAS，最后将
`worlds.enabled`、通用 `auction_commands` 结果和追加式 Admin 审计在同一 PostgreSQL
Serializable 事务中提交。任一 blocker、版本冲突或审计失败都必须整体回滚。SQLite Lite 可以读取
业务世界状态，但不得伪装提供这项完整档 mutation。

### I-04 PostgreSQL 与 SQLite 使用不同的并发机制

- PostgreSQL 竞价和关键生命周期写入使用 Serializable 隔离，并配合明确的行锁；
  可重试的序列化冲突最多重试 3 次，超过后返回可识别的依赖/冲突错误。
- 普通/代理/慢速竞价的 world/auction 只读门禁使用 `FOR SHARE`，lot 与资格、代理、账本仍持有
  各自排他锁；按 world → auction → lot 取得锁。共享锁必须与启停/生命周期更新互斥，不能
  替换为普通读取或允许这些状态字段被同时更新的弱锁。
- PostgreSQL 结算事务提交只重试明确映射为序列化冲突或死锁的错误；连接中断、约束失败和
  其他提交错误必须保留原分类并立即失败，不能重放未知提交结果。
- PostgreSQL 的队列领取、游标推进等路径可以使用 Read Committed，但必须通过
  明确锁、唯一约束或条件更新保护领取和状态变化，不能把较低隔离级别扩展到竞价
  正确性路径。
- SQLite Lite 使用明确的串行写事务，开始写入前取得 RESERVED 写锁，应用层保证
  每个角色至多一个实例、Worker 并发为一；SQLite 不模拟 PostgreSQL 的 FOR UPDATE。
- 所有后端都必须失败关闭非法状态迁移，不能用“最后一次写入获胜”掩盖并发冲突。

### I-05 成功竞价的写入集合是原子的

成功竞价必须同时具备竞价事实、状态版本、追加式审计记录和持久化事件/outbox。
任何一项写入失败都必须使整个命令失败。外部论坛或邮件调用不能放在本地事务内，
也不能在本地提交失败后提前告诉用户成功。

### I-05a Admin 装备录入的敏感边界和原子集合

Admin 装备录入读取外部 URL 或 canonical JSON 时，raw 装备 key 只能存在于短生命周期
的应用对象中；它不得进入日志、HTTP DTO、命令账本、审计 metadata、事件 payload 或
普通装备字段。提交数据库前必须用配置中的数据密钥封装为带 nonce、key id 和 fingerprint
的密文信封，数据库只保存信封。

唯一允许离开该密文边界的公开数据路径，是 P09/P11 lot JSON 中既有的
`game_equipment_url` 字段：服务端必须先核对当前公开 lot、装备 owner、world、无 key 投影和
密文信封，再在短生命周期内解密并构造固定 HentaVerse origin 的完整 URL。该显式产品边界允许
10 位 key 出现在 P09/P11 响应体和浏览器 `href` 中，但不得进入日志、审计、事件、命令账本或
持久化缓存；两条响应必须使用 `no-store` 与全局 `no-referrer`。SQLite Lite 必须保留 lot 读取并
把该字段留空，不能返回看似可点击的 keyless URL。浏览器向 HentaVerse 发起的用户主动导航会按
外部装备协议携带 key，这是该功能的显式交付边界。

PostgreSQL 完整档的录入必须在同一 Serializable 事务中完成命令账本占位、世界级目标
锁、玩家资格检查、装备与密文、submission、pending lot、拍卖版本、审计记录和实时事件；
任一项失败都必须回滚。外部来源读取发生在写事务之前，来源失败不能留下半个 lot。相同
command_id 只重放脱敏结果，不得再次抓取来源或重复写入事实；SQLite Lite 必须明确返回
能力缺失，不能伪装成成功。

唯一录入目标可以指向 Draft 或 Preview，但必须与拍卖的真实状态精确匹配。Preview 不是冻结
目录：此阶段新增的 Admin/MM 装备仍属于本期，并必须在同一事务把新 lot 追加到
`auction_preview_items`、推进 `preview_revision` 后再发布目录重读事件。正常生命周期中只有 Preview
正式进入 Live 的事务才能创建下一期 Draft 并移动录入目标；唯一例外是 Preview 被取消时，取消事务
必须建立后继 Draft，避免该世界失去可用录入目标。与开拍或取消并发的录入要么完整进入原场次，要么
在目标切换后完整进入下一期，不能跨两期拆分。

### I-05b Admin 论坛预览是有界的纯本地操作

Admin 论坛预览请求必须在 HTML 与纯文本中恰好选择一个非空输入，输入体积不得超过 2 MiB。
解析器只能在内存中生成 catalog、posts、warnings 和命令预览；解析失败必须返回明确的输入
错误，不能产生数据库写入、远程抓取、回帖或“已发布”的成功信号。预览输出不得包含 Cookie、
密钥或其他外部认证材料，远程发布仍必须由后续 Worker 的已提交事实和外部投递边界负责。

### I-05c 服务心跳是有界的可过期安全投影

服务心跳必须校验服务/职责闭集、实例与版本边界、正的 observation sequence、tracing
计数和 TTL；旧 sequence 不能覆盖新快照。数据库读取只返回未过期且通过安全投影校验的
字段，details 原文不进入 dashboard。健康聚合缺少任一 required service 时必须降级；尚未
迁移的 owner section 必须返回 `unknown`，不能用默认值伪造健康。

## 领域状态和金额

### I-06 状态机只允许声明过的迁移

拍卖状态迁移必须由领域层集中定义。非法迁移、未知标的、过期版本、无效身份和
不满足资格的命令必须明确失败；HTTP 层、Worker 和存储适配器不能各自复制一份
状态机或静默修正输入。

初始生命周期至少覆盖创建、预览、开放、进入最后阶段、取消、结算和恢复所需的
边界。具体状态集合和迁移表以领域模块及本仓库冻结的版本化契约为准，新增状态必须
同步更新事件、路由、数据库和运维文档。

PostgreSQL 完整档还必须在数据库边界保护拍卖聚合身份不可变、前序拍卖序号早于后继，
以及每个世界至多一个处于 Draft/Preview 的可录入拍卖，且 `auction_intake_targets` 恰好指向该
聚合并保存相同状态。后两者是跨行不变量，使用跨状态唯一索引和延迟约束触发器，允许生命周期
事务先推进状态、再同步或移动 target，但提交时必须一致。
论坛同步入队不属于这些领域触发器；它由 Worker 从已提交生命周期版本发现并持久化领取。

`worlds.enabled` 是新业务进入当前生命周期的权威门禁。关闭时必须拒绝存在
Draft/Preview/Live/Final/Settling 拍卖或活动未完成轮次的世界；成功关闭后，Public 当前目录、
竞价写入、当前实时流和 Worker 新一周 bootstrap 都不得继续处理该世界。已经提交的 settlement、
fulfillment、delivery 与 recovery 义务仍按持久队列排空，不能因关闭世界而被丢弃。Public 历史读取
只为 `settled` 拍卖保留例外，即公开谓词为 `world.enabled || auction.status == settled`；该例外不能
扩张到 cancelled、preview 或其它活动状态。

### I-07 出价计算必须遵守冻结的离散规则

基线规则包含以下不可被浮点或近似算法替代的语义：

- 外部普通出价、代理上限和规则默认金额使用 1000 为信用网格；历史 lot 可以使用任意正整数
  起拍价，并允许首次或固定增量自动代理事实保留同一网格余数；
- 新普通出价、新建/降低代理上限及自动代理的新竞价事实最高为 1B（1,000,000,000 Credits），
  包含恰好 1B；超额外部输入必须失败，不能自动缩为上限。既有金额值对象、历史授权/事实及幂等
  重放继续保留原值，不截断结算总额；自动代理的有效竞价上限不得超过 1B，到达上限时保留已有
  代理的同价优先级。下一口数学最低价可超过 1B，以保持读取契约中的严格抬价关系，但不得作为
  可提交金额，浏览器应显示上限状态并禁用新出价；
- 默认起始金额为 1,000；
- 默认使用 50,000 PPM 的经济增量，按信用网格向上取整且最低增加 1,000；
- PPM 明确设为 0 时使用冻结的最低增量（默认 30,000）；
- 代理竞价的反超增量为 1000；
- 软截止窗口和延长时长均为 1 分钟；
- 取消截止边界为开始前 24 小时。

实际实现必须以 `auction-domain` 的领域常量、金额类型和边界条件为权威。所有金额都必须
为正且不溢出；外部普通出价、代理上限、论坛命令和规则默认值额外校验信用网格。存量
lot 起拍价/当前价与自动代理事实不得被静默取整：固定增量直接保留原网格余数，PPM 分支
先把当前价向上折算为信用网格单位，再把向上取整的整千增量加回原始当前价，同样保留余数。
软截止只允许把结束时间向后延长，不得缩短已经确定的有效结束时间。

### I-07a 结算策略与命令重放必须可从权威事实重建

`hold_curve_v1` 必须启用补偿池，`legacy_zero_v0` 必须关闭补偿池；配置不能组合出
会计算税额但不写资金池账本的第三种语义。当前格式的完成命令重放必须使用冻结策略、首次结算
资金池和锁内 lot 事实重新计算计划及义务，并与结算摘要和账本 payload 全量相等；只满足 payload
内部加总守恒不足以证明结果合法。任何漂移都失败关闭，不现场修补或重复执行结算。

### I-07b 拍品展示编号与内部顺序必须分离

`lots.lot_number` 是场次内唯一、正数且不可变的内部全局顺序，用于锁序、预览物化、历史游标和
审计关联；它不得因为展示规则改变而重排。`category_number` 是与不可变 `category_key` 组合使用的
对外编号，在每场拍卖的每个装备分类内从 1 独立递增，撤回或重挂不得复用旧号。数据库必须同时
约束 `(auction_id, category_key, category_number)` 唯一并校验分类键与 equipment category 一致。
所有写命令仍以 `lot_id` UUID 定位，不能把跨分类可重复的展示编号当作身份或 cursor。

论坛主题一旦开始远端发布，其编号方案必须冻结；迁移前的主题继续解释原全局编号，新主题使用
分类编号。不得把既有帖子或命令账本中的 lot code 静默重解释为另一件装备。

## 外部副作用、Worker 和缓存

### I-08 外部写入发生在本地提交之后

论坛发帖、邮件发送、通知广播和其他外部副作用只能消费已提交的本地事实。外部
失败必须进入可重试、可观测的任务状态，不能撤销已接受的竞价，也不能把外部成功
当成本地事务已经提交的证明。

普通邮件入站的 take/return/notice 即使已经写入 delivery outbox，也必须等 `inbound_mail` 与
`mail_commands` 完成事实提交，并由入站决定中的精确 delivery ID 授权后才能领取或进入 sending。
复用 MailIntake origin 的论坛通知通过同 command ID/world 的论坛 notify owner 证明，不能用
任意模板名绕过门禁。投递命令摘要与邮件内容摘要属于不同命令，不能要求二者相等。

### I-09 Worker 进度必须持久化

Worker 的命令账本、outbox、消费游标、领取状态、重试次数和失败原因必须能在重启
后恢复。内存定时器、内存游标或单进程广播不能作为唯一事实。重试必须幂等，超过
策略上限的任务进入明确的失败/人工处理状态。

邮件读取预算内只返回已校验的连续详情前缀；列表未定位到 cursor 或完整末尾时不得推进。
重读已取件邮件时先核验持久化完成证明，不能用已变化的远端资产重新解释已完成业务。既有
事实和账本仍严格保留原摘要；缺失或损坏证明不能跳过，游标只越过已获证明或本轮成功提交的前缀。

### I-09a Operations alert 的通知冷却不能早于入队证明

Operations alert 批次创建时即持有第一次 claim，`claim_fencing` 与 `attempt_count` 从 1 开始；
lease 过期或 fencing 不匹配的 Worker 不得确认入队或释放 claim。普通 delivery Create 失败只把
批次释放为错误码为空的 `pending` 并设置持久化退避，identity 不一致立即进入
`failed/dependency_mismatch`，第三次失败或第三次 lease 过期进入 `failed/retry_exhausted`。

signal 的 `last_notified_at`、`next_notify_at` 和版本只能在批次、delivery 同一事务确认入队后推进；
领取或 Create 失败不能提前消耗 cooldown。delivery 的外部失败与人工复核只投影为稳定的
`delivery_failed` / `delivery_manual_reconcile`，不得把任意远端错误原文写入告警健康投影。

scan、dispatch 和 reconcile 必须独立调度并各自受有界 deadline 约束；一个 owner 的读取、领取或
写回失败不得阻塞另外两类 owner。默认一分钟 cadence、阈值、批次窗口、冷却、重试与正文边界属于
当前运行契约，修改时必须显式评审告警灵敏度和通知频率。

### I-09b MoogleMail 破坏性动作必须有前后双重证明

return/take 只能在已重新读取的详情页上执行，动作前必须证明 sender 与当前
delivery 收件人一致。take 还必须精确比对装备 ID 集合、Credits、COD、附件条目数和
claimable 动作；只检查按钮存在不构成资产证明。前置证明失败时不得发起 POST，也不得将
未知远端状态当作可自动重试的确定失败。

POST 后，return 必须满足两种冻结证明之一：当前协议的唯一只读 `[RTS]` 回执需要与动作前
To/From/Subject、正文、装备 ID、Credits、COD 和附件数逐项绑定，精确证明收发件人反转、固定
MoogleMail 发件人、标题前缀、正文退回后缀、资产不变且 return/take 动作都已消失；兼容协议则必须
从结构合法的收件箱中证明目标 `mid` 在所有 `href`/`onclick`/`action` 引用和结果行中均已消失。
take 必须证明 To/From/Subject 不变、资产与 COD 归零、claimable 消失，且动作前正文是动作后正文
的前缀，预期的 `CoD Paid` 和 `Removed attachment` 回执数量与金额精确一致。仅出现“成功”文字、
`[RTS]` 标题或动作按钮消失都不足以证明完成；后置证明失败必须进入 `manual_reconcile`，不得盲目重发。

### I-09c MoogleMail 明确发送失败必须保留资产后果

send 只能从能证明为无收件人、正文、附件和 COD 残留的干净 compose 页开始；无法证明时
必须进入 `preflight_unproven` 人工对账，不得覆盖或自动清理可能属于上一次结果未知的资产。
send 失败只能根据远端 body 中的可见协议文字分类，script、style、noscript 和邮件
textarea 中的同名短语必须排除。明确的收件人不存在已有拒绝证明，但仍必须保留唯一、
合法的退信 `mid`、`take_required` 和稳定 reason code，并在退信资产取回前进入人工对账，
不得直接终结为普通失败。

Worker 收到明确 `recipient_not_found` 后，必须只在当前领取轮次内暂时排除同一
`(world, recipient)` 的后续 delivery，并继续处理其他玩家。排除集合不得跨 world、不得批量改写
未尝试 delivery，也不得形成永久黑名单；下一轮允许重新评估。首条失败 delivery 仍独立保留
`manual_reconcile`、退信引用和资产回收责任，禁止自动重发。

明确的邮资拒绝只在 compose 中从未暂存 Credits、装备或 COD 时可以作为确定失败；任一
资产暂存已经通过证明后，拒绝结果必须进入 `manual_reconcile`，禁止自动重发。失败
transport 元数据只能是有界的安全 object；storage owner 必须覆盖而不是信任其
`safe_detail` / `outcome_unknown`。普通日志、数据库和 DTO 不得保存原始响应、Cookie 或请求
payload；但业务拒绝、业务失败、处理超时以及 HTTP/解析异常可以按
[`docs/operations/mail-diagnostics.md`](../operations/mail-diagnostics.md) 写入独立的加密异常证据，
证据保存失败不得改变原有退回、投递、重试、游标或幂等结果。

Delivery claim 必须在进入 `sending` 前按 payload world 检查 scoped/shared 凭据。当前无凭据时
只能用完整 lease token 和 fencing 从 `claimed` 释放回 `pending`，不能写成终态失败；非法 payload
仍必须越过 availability gate 进入确定失败路径，避免永久滞留。进入 `sending` 后发生的凭据竞态、
网络、响应边界或协议依赖失败不得收口为普通失败。

可能超过初始 lease 的多步外部写必须周期续租，并在每次续租和最终写回中携带同一 lease token
与 fencing version。Worker 只能在有空闲执行槽时领取下一条，不能让批次内排队时间消耗新 claim
的租约。完整外部调用与终态写回必须分别有界；请求超时属于结果未知。续租失败后不得以 stale
claimant 保存成功或确定失败；只有原 fence 仍有效时可以写入结果未知。过期 `sending` 必须在额外
sending grace 后进入 `manual_reconcile`，禁止重新领取并盲目重发。

Delivery 每轮完成后必须使用有界 `poll + [0, jitter]` 再唤醒，避免多实例固定整点抢占。单轮
claim、持久化或外部处理错误必须由持久化状态保留重试边界并在下一轮继续，不能因此终止整个
Worker；循环任务异常退出仍必须失败关闭。

### I-09d Delivery 状态事实与审计不能分叉

Delivery 的创建/准备/激活、claim、进入发送、释放、成功、失败、结果未知、过期发送隔离和
Admin 人工动作必须在同一 PostgreSQL 事务追加安全审计；审计失败时状态变化必须回滚。两阶段
激活必须同时持久化独立的 activation command ID、请求摘要和激活时间，所有后续状态上的幂等
重放都必须匹配该身份；幂等重放本身不得重复追加审计。续租不改变业务状态，不逐次生成审计噪声。

### I-09e Fulfillment 与 Recovery 不得共享易失调度边界

PostgreSQL fulfillment 和 recovery 必须使用各自的持久化领取循环、批次、并发、lease、单项总时限、
重试和 `poll + [0, jitter]` cadence。一个 owner 的领取、依赖或持久化失败只能影响自身下一轮，不能
阻塞其它 owner 或终止整个 Worker；循环任务 panic 或意外退出仍必须失败关闭。

Fulfillment 只能按并发波次领取，避免尚未开始的工作提前耗尽 lease。Recovery 必须按
`case → return → action → relist → refund` 依次尝试全部 phase；单个 phase 失败不能跳过后续 phase，
action 仍由 fencing 和持久化重试保护。默认 60 秒轮询、5 秒抖动、100 条批次、8 并发、
60 秒 lease/重试和 30 秒单项时限属于当前运行契约。

### I-09f Auction 生命周期与结算必须独立于集成轮询

Auction 生命周期和结算 owner 必须立即执行首轮，再使用自己的 `poll + [0, jitter]` cadence；邮件
唤醒、论坛、realtime 或其它集成 tick 不得额外触发 Auction，也不得延迟它。生命周期与结算各自
拥有完整 batch 上限并按共同并发波次领取；一类 claim 或单项失败仍必须尝试另一类，且只影响主循环
下一轮。默认 60 秒轮询、5 秒抖动、每类 100 条、PostgreSQL 并发 8 和 30 秒单项总时限属于当前
运行契约；SQLite Lite 的有效并发必须固定为 1。

### I-10 缓存不是权威数据源

缓存只能保存可重建数据。竞价金额、资格、幂等账本、锁、会话真值、审计证据和
结算义务必须由数据库或明确的权威服务决定。缓存禁用、过期、丢失或不可用时，
命令结果不能改变。

## 审计、凭据和事件

### I-11 审计与事件只允许追加

审计记录、领域事件和 outbox 记录不可更新或删除来“修正历史”。更正必须追加新的
事实并说明关联关系。流序号不可复用，回放必须按序读取；检测到无法安全续读时
要求读取权威快照。

Admin 审计读取只能消费追加式 `audit_records` 的安全投影。筛选和分页必须有界且保持
时间/ID 的稳定排序；数字页的总数与记录应来自同一读取快照，keyset 页不得把未知总数
伪造成精确值。metadata 必须是 JSON object，异常存储行应失败关闭，不得把原始数据库
字段或潜在凭据直接发送到浏览器。

### I-12 敏感材料不得离开安全边界

会话签名密钥、论坛 Cookie、邮件 Cookie、数据库密码和其他凭据不得进入日志、
HTTP DTO、事件 payload、快照、错误消息或提交的配置。错误只能返回可操作的
分类和关联 ID，不能回显敏感 TOML 原文或连接串。

`mail_cookie` 与 `forum_cookie` 各自全局最多一条 active 记录，最终约束必须由 PostgreSQL 唯一索引
保护，不能依赖可竞态的“先查后写”。Worker 必须按凭据类型持有进程生命周期 session advisory
ownership；同类第二个 Worker 获取失败时应拒绝启动。远端确认失效后只允许幂等标记当前 Cookie
为 `invalid`，不得自动轮换到第二条。ownership 保持到 Worker 退出，因此替换 Cookie 后必须重启
Worker 完成交接。`.keyenv` 的 `hv-ck-key2` 只供显式隔离验证，不能成为运行时后备。

### I-12a Public WebSocket 不能成为无界资源或第二个命令入口

Public WebSocket 只允许服务端推送已提交的公开事件。每条连接必须有 4 KiB frame/message 上限、
60 秒 Pong 存活期限和 5 秒逐次写入期限；客户端 Text/Binary 必须以稳定 1008 reason 关闭，不能
被解释为竞价或管理命令。Hub 慢消费者、事件合同错误和进程 drain 分别使用稳定的 1013、1002、
1001 reason；底层读写或超时失败直接结束，不能伪造业务原因。

discovery 的 128 槽是 Public 进程本地、可丢弃的资源保护，不得限制普通单拍卖连接，也不得进入
竞价正确性。连接尝试限流必须与 mutation 配额分账；公开状态预检由权威拍卖读取端口决定，不能
从事件历史猜测。settled 拍卖不接受新连接，但已建立连接可消费其终态事件。

## 能力档位和模块 owner

PostgreSQL 是完整生产档；SQLite 是本地、演示和明确单主机低并发的 Lite 档。
标为“两者”的能力必须在两个后端保持声明的领域正确性；标为“完整”的能力可以
只由 PostgreSQL 提供，但 HTTP/API 必须能清楚报告 Lite 不支持，而不是返回伪成功。
具体对象档位见 docs/contracts/schema.md。

写入责任按以下边界分配：

| 责任 | owner |
| --- | --- |
| 领域规则、金额和状态机 | auction-domain |
| 用例、事务编排、命令端口 | auction-application |
| 稳定 DTO、事件信封和错误契约 | auction-contracts |
| SQL、事务、锁、迁移和命令账本 | auction-storage |
| HTTP/WS 映射 | auction-http |
| 重试、游标和外部副作用编排 | auction-jobs |
| 外部论坛、邮件和网关认证 | auction-integrations |

任何新增表、事件、写入查询或路由都必须只有一个 owner，并在架构文档中记录其
属于完整档、Lite 档或两者。

## 验收边界

格式、编译、静态分析和契约审计只能证明结构没有明显漂移，不能据此宣称生产行为已经验收。
除非用户明确要求，不新增、不运行单元测试、集成测试、功能测试、端到端测试或浏览器测试；
真实外部服务、并发、非空生产数据和切换前验证需另行授权后执行。
