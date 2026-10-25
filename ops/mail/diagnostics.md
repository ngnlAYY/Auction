# 邮件诊断与审查证据

[文档导航](docs/README.md) · [加密证据](crates/auction-integrations/src/clients/mooglemail/diagnostics) · [离线导出和显式回填](apps/auctionctl/src/bootstrap/runtime/mail_diagnostics) · [审查与诊断端口](crates/auction-application/src/ports/mail)

本页区分加密原始证据、脱敏审查投影与业务完成证明。PostgreSQL 完整档 Worker 可为业务拒绝/失败、解析或 HTTP 异常、
处理超时留证；普通成功邮件不归档原文。原始正文、Cookie、请求表单、token 和装备 key 不得进入普通日志、命令结果或普通
数据库/DTO；Admin 只能读取下文限定的脱敏展示投影。

## 本页导航

- [普通日志](#普通日志)
- [保存内容与边界](#保存内容与边界)
- [配置与离线导出](#配置与离线导出)
- [Admin 审查展示投影](#admin-审查展示投影)
- [已知 MID 的履约读取](#已知-mid-的履约读取)
- [主动 ComposeSend 的 Sent 证明](#主动-composesend-的-sent-证明)
- [定期三页维护](#定期三页维护)
- [装备读取失败诊断](#装备读取失败诊断)
- [历史资产处置核对](#历史资产处置核对)

## 普通日志

按 `world_id`、`mailbox_key`、`external_mail_id`、stage、operation 和结果关联日志；装备失败再带 equipment ID/附件位置。
定价拒绝保留 `reason_code`，有界 `pricing_diagnostic` 包含 `detail_code`、`body_bytes`、`token_count`、`field_index`、
`attachment_count`，这些确定性字段可进入 `decision_metadata`。随机 `evidence_id`（UUID）和留证回执不得参与幂等比较。

| `evidence_status` | 含义 |
| --- | --- |
| `retained` | 本次完整正文已加密保存 |
| `body_unavailable` | 未取得正文或完整响应；不能以 partial body 伪称完整 |
| `disabled` | 留证开关关闭，安全日志仍输出 |
| `prepare_failed` / `save_failed` | 加密准备/保存失败，只输出安全分类与 warning |
| `completion_unconfirmed` | 租约取消或外层超时中断等待；本地写入可能仍在完成，既不证明已保存也不证明无文件 |

留证不改变退回、投递、重试、游标或幂等结果。UUID 不保证文件永久存在：保留期/容量淘汰后导出为 `not_found`，
不会改写当时的日志状态。

入站发件人排错：旧版本的 `operation=detail/category=identity_mismatch` 可能来自列表显示名被截断，
不是详情完整发件人无效。当前 integrations 仅采信详情完整名称，MID/world/时间绑定、详情结构与资产校验仍保留；
列表显示名不参与身份比较或 allowlist。升级获准发布的 Worker 后，旧待读项按原 MID 恢复，无需改写摘要、删除队列、
跳游标或重发已完成邮件。白名单须填写详情完整名称；未命中时按 `configuration` 保留队列并失败关闭，
不会领取、退回、回复或越过该邮件，后续处理等待调整白名单。本文步骤不授权部署或重启。

### Admin 入站处理观测

在 `/mails` 输入精确邮箱并加载/刷新轮询状态，可查看“入站处理诊断”。已完成邮件游标只展示安全数字 MID；
当前阶段/MID 与最近失败定位来自 Worker 只读运行快照，不从待读队首猜测。失败目标应与本页普通日志中的
world/mailbox/MID 及加密证据关联。界面同时标注读取、Worker 观测与阶段更新时间；这不是自动刷新或自动恢复动作。

Worker 未升级、control 不可用或尚未观测时明确显示对应空态，数据库侧安全已完成 MID 仍可读取。
最近失败仅在当前 Worker 进程内保留，重启清空；原始 cursor、claim token、Cookie、正文及装备 key 不返回浏览器。
诊断能力不新增迁移；上线须按既有 schema/制品门禁受控更新 Worker 与 Admin，不由页面自动部署或重启。
HTTP 与本地 control 字段见[邮件契约](docs/contracts/routes.md#邮件审查与轮询)。

### 显式单封跳过的运维边界

当前没有通用自动跳过接口。临时跳过须另行确认精确数据库、mailbox、MID、原 cursor 与待读序号；
先按认证的加密详情核实为零资产邮件，并告知该封不会自动完成注册或收到回复。行级补偿证据包含原邮箱状态、
完整目标待读记录、计划、SHA-256、加密原证据及提交回执；目录 `0700`、文件 `0600`，mutation 前验证并同步备份。

一次性维护在同一 Serializable 事务中锁定邮箱及待读窗口，核验当前版本、原游标、无活动 claim、目标尚未入站且窗口
只有已确认的单封邮件；仅移动 cursor、递增版本、移除目标待读项，并追加 `mail.operator_skip_unprocessed` 审计。
审计绑定稳定 `command_id`、请求摘要、备份摘要、原/新 cursor、原/新版本与证据 ID；不创建入站成功事实或 delivery。
若目标前后仍有其他待读邮件，不能直接前移总 cursor。另行确认的单封隔离须在同一行锁内备份完整窗口，
仅移除指定 MID/序号、递增邮箱版本，保持 cursor 不变，并逐项核验其余 MID、顺序与记录均未改变；追加
`mail.operator_quarantine_pending` 审计，同样绑定原/新版本、备份与请求摘要。状态在预检后变化时必须中止，
确认旧命令未提交后才能建立明确替代它的新计划，取快照、备份与 mutation 持续持有同一锁。

提交回执不明时只按原命令核对审计，不能换 ID 盲目执行。保留原失败历史与调度，后续仅观察正常轮询，
不自动扩大跳过范围、回退游标、覆盖原摘要、重发或重启；补处理该封邮件仍需另行确认。

## 保存内容与边界

留证分两类：HTTP 异常保存实际取得的响应字节（最多 8 MiB）及真实 status，未取得 status 为 unknown，解析层只持有已读取
正文、HTTP status 为 unknown 时不能伪造 `200`；业务拒绝、业务失败和处理超时保存实际提取的 JSON（`body`、`subject`、
`sender`、附件外部 ID/key、`credits`、`cod`、`returnable`、`claimable`、`received_at`、`content_digest`），`body` 保留
Unicode 空白，以便复核类似 `<equipment-url> start 50k` 的输入。

请求 Cookie、请求表单和未取得的响应不留证；响应 HTML 中的 token 随整体加密。原文只存在独立文件，不跨批次保存 HTML，
不恢复启用前未留存的历史原文。

父数据密钥经固定独立 HMAC 域派生诊断子密钥，以 AES-256-GCM 加密；`key_source`、`key_id`、`outcome`、`context` 组成安全
public header，完整 AAD 认证元数据。优先 `credential_data_key`，缺失时用 `equipment_data_key`，不直接复用凭据 AES 密钥。
目录 `0700`、文件 `0600`，保留 1–365 天、最多 1024 文件；只在写入时按容量/保留期清理，没有独立定时清理或自动重放。
证据文件不新增 schema，不替代业务事实、账本或游标。

## 配置与离线导出

Worker 主配置可使用以下区块，目录相对 Worker TOML 解析。父目录必须已存在，路径不能经过符号链接；现有证据目录必须归
运行账号所有且权限为 `0700`。私有文件适配器只在 Unix 上启用，其他平台返回 `unsupported_platform`，不会降级保存。

```toml
[worker.mooglemail.diagnostics]
# 可选：是否保存异常原始数据；默认 true。
enabled = true
# 可选：诊断归档目录；相对 Worker TOML 解析，默认 mail-diagnostics。
directory = "mail-diagnostics"
# 可选：保留天数；范围 1—365，默认 7。
retention_days = 7
# 可选：总字节上限；范围 16 MiB—1 GiB，默认 268435456（256 MiB）。
maximum_total_bytes = 268435456
```

使用 `auctionctl` 按 UUID 离线导出，不连接数据库、Worker、MoogleMail 或其他远端服务：

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  auctionctl mail-evidence export <evidence-uuid> \
  --output /secure/auction-evidence/<evidence-uuid>.json
```

可用 `--key-source credential` 或 `--key-source equipment` 指定历史父密钥来源；省略时优先 credential，缺失时使用 equipment。
输出路径使用 `create_new` 和 `0600`，已存在文件不会覆盖，命令不把正文打印到 stdout；证据已被容量或保留期清理时返回
`not_found`。历史导出必须继续保留对应旧父密钥备份，当前实现不会自动轮换或迁移诊断密钥。

## Admin 审查展示投影

完整 `MailMessage` 首次写入 `inbound_mail` 时生成脱敏审查投影。列表增加可空 `sender`/`subject` 及 `has_message_display`，
详情才返回 `message_display`；snapshot 固定 schema/redaction version，正文最多 64 KiB、JSON 最多 128 KiB、附件外部 ID 最多
1000 条，Credits/COD 保留为非负数；装备 key、URL key 片段、认证邮件正文和认证相关敏感内容均隐藏。该投影是非权威展示
数据，不参与入站幂等或 cursor，普通 replay 不会补写历史 NULL。

历史回填只接受认证的 `parsed_message` 诊断证据，并要求 world、mailbox、external mail ID 和 content digest 精确匹配，再以
CAS 和追加审计写入；相同幂等身份但内容不同必须冲突，没有匹配证据的历史邮件显示未留存。回填不会把证据变成业务权威事实。

`auctionctl mail-evidence inspect-sent <obligation-id> --output <path>` 是 PostgreSQL 完整档的只读 Sent 诊断，范围仅限
`withdrawn_return` 单件装备、零 Credits、`COD=0` 的义务。命令按页数和总 deadline 读取，要求唯一且精确匹配 world、
recipient、canonical subject 和装备 ID，并把结果标记为 `time_unverified`；它不是通用 payment 核对，也不是已送达证明，
`collected` 只表示结果尚未取得充分证明。它不会自动 reconcile、send，也不领取或修改 delivery、fulfillment、lease 或远端
邮件；输出文件必须是新的私有路径。

## 已知 MID 的履约读取

履约已持有可信发送成功引用时，按 world 的 Sent 列表逐页定位精确 MID，命中合法列表行后立即读取并核对详情；
无关尾页不参与该 MID 的正向身份证明。列表和详情的 MID/world/收件人/主题/时间与资产矩阵校验不变，
只有确实到尾页仍未命中才返回未观察到；页数耗尽返回不完整扫描错误，不作为已领取或未发送证明。
这避免完整历史扫描反复耗尽履约的有界阶段预算。履约使用独立新鲜只读轮次，证据不早于本次 claim，
不复用旧共享观察，也不失效其他读取者；全局并发、内存、owner 与代次校验不变，
资金/库存授权仍取得原独占 session、新鲜屏障及最长 30 秒证据期限，不消费普通缓存作为资产提交授权。
该路径不适用于新发送基线、未知候选搜索或写后新增唯一证明，也不重排队历史未知发送。

健康核对须结合 delivery 终态与原发送证明：`evidence_unavailable` 表示读取依赖暂不可用；
`evidence_not_observed` 包括发送尚未完成、目标暂未观察到或邮件资产尚未领取，不能单凭该码区分三者。
COD 付款与货款领取继续等待游戏内真实动作；业务阻塞的卖方款须等待前置义务满足，条件退款不视为已授权付款积压。
保留真实 `manual_reconcile` 和原 attempt/fence，不为清除健康告警重寄、清零历史计数或强制结清。

## 主动 ComposeSend 的 Sent 证明

主动 ComposeSend 固定四步验证：①发送前读取 compose 并证明为空表单；②对 Sent 做覆盖所有 MID 的完整有界
baseline；③发送后执行独立有界 Sent scan 并要求收到新 MID 且字段精确匹配；④再次读取 compose 并证明回到空表单。
前两步在持久 sending fence 前、同一独占凭据 session 内完成，失败只释放有效 claimed，不消耗发送间隔。
写后失败或页面 `mail_state` 与待领取资产矩阵不一致仍标记结果未知。成功条件是
新增且唯一的真实 MID，并同时精确匹配 world、recipient、subject、canonical body digest、Credits、COD 和装备 ID；0 资产
通知也要求 0 资产 Sent 证明。写后资产不符、秒领取、无匹配、多匹配、未扫完或超时均标记结果未知并进入 `manual_reconcile`，
不自动重寄；成功页、空 compose、本地 UUID 都不能替代证明。该规则覆盖静态通知、注册/重绑 key、运维告警及所有结算/返还
金钱和装备。

`sent_scan_page_limit` 表示完整扫描尚未到尾页就耗尽本地页数，`sent_scan_timeout` 表示本地总扫描预算耗尽；
不再把这两类预算错误统称 `remote_temporary`。日志只含 world、闭集阶段、上限和计数，不含 MID/收件人/正文。
基线句柄不可克隆、不可跨 session 缓存且只消费一次；发送后复用完整基线证明新增，不因上调预算跳过完整性。
历史人工核对记录保持原状，`write_started=false` 或 Sent 零命中也不自动授权重发。
所有新 Send 在准备阶段冻结实际主题、正文摘要、Credits/COD/装备矩阵和安全凭据绑定；
`000061` 与 Sending 同事务登记发送尝试。实际 POST 消费该次已渲染请求，不能在 fence 后重新读取模板或登录 key。
恢复后的 `sent_recovered_frozen_v1` 与原实时证明保持区分；下游通过数据库原尝试及唯一 MID 绑定核验，不只检查标签。

外层 delivery timeout 默认 180 秒、sending grace 默认 300 秒；启用 MoogleMail 时 timeout 必须大于两倍 fetch budget，
读取复用既有 `maximum_pages` 和 `fetch_timeout`，不新增配置字段。成功 metadata 为 `sent_verified=true`、
`external_reference_proven=true`、`proof=sent_exact_payload_v1`，真实引用为 `mooglemail:world:send:mid`。
[sender/proof.rs](crates/auction-integrations/src/clients/mooglemail/sender/proof.rs) 编排 baseline 与新增 Sent 核验，内容解析由
[parser/reads/sent/detail.rs](crates/auction-integrations/src/clients/mooglemail/parser/reads/sent/detail.rs) 的
`MoogleMailParser::parse_sent_content_proof` 负责，Sent 详情接受 `mail_state=1`（仍有待领取）与 `mail_state=2`
（已领取完毕，且必须零 Credits/零 COD/零附件）；storage 成功写回前执行 defense gate。Return/Take 使用独立 action proof，
不属于主动 ComposeSend。

Admin 人工 reconcile 不能把 `payload.operation=send` 与裸 `external_reference` 填成 `succeeded`，后端会按既有 invalid 条件
拒绝，Admin Web 隐藏该成功选项；`retry` 和 `failed` 仍需显式管理员命令与审计，Return/Take 保留独立人工核对路径。
收件详情的“关联邮件任务”表仅用于查看与手动重发：`operation` 为 `Send`、`Return` 或 `Take`，按钮分别为“重新发送”
“重新退回”“重新领取”；只有 `failed` 或 `manual_reconcile` 可操作，`pending`、`claimed`、`sending`、`succeeded`、`prepared`
均不可重试。确认后复用既有 delivery retry/reconcile（`action=retry`），携带稳定 `command_id`、CSRF 和
`expected_fencing_version`；成功文案只表示“重新排队”，结果未知时提示先核对 MM。

### 附件装备状态准备

发送包含装备时，fence 后先证明 compose 干净，再读取本世界七类 Armory organize 库存；
所有请求 EID 均唯一命中且未穿戴后才可修改。正常装备不执行 Armory POST，无装备邮件不读取 Armory。
同分类需要处理的目标可批量提交，但不得夹带未请求装备。状态只来自精确 EID 行，
先剥离元数据中的实际显示名再解析原生状态图标，避免自定义名称中的图标触发误判。

- 固定写目标为本世界的 `?s=Bazaar&ss=am&screen=organize`，表单使用新鲜 `postoken` 与重复的 `eqids[]`。
- 仓库状态使用 `set_stored=-1`；保护和锁定共用 `set_locked=-1`。不使用 toggle，不改变置顶、穿戴或其它装备。
- 每个待修改分类写前重新读取；每次 POST 后回读同分类，核对目标集合及所需状态并取得新 token。
  出库后仍受保护时继续解除；两个条件均满足才进入附件暂存。实际修改后再次 GET compose，核对干净状态并刷新邮件 token。
- 对应安全操作码为 `inventory_organize`、`equipment_unstore`、`equipment_unlock`，修改阶段为 `equipment_preparation`。
  token、Cookie、原始装备页面和完整表单不得进入普通日志或 DTO。

读取不完整、目标缺失／重复、页面身份不符、已穿戴、状态无法证明均停止本次发送。
Armory 写后未生效、断连或回读失败按 `write_unknown` 保留人工核对；明确会话失效保留 `session_expired`，
但仍属于未知写入，不换 Cookie 或重发。此时装备可能已出库／解锁，也可能尚未附加，
不能把空 compose 或“未收到邮件”当作可以直接重试的证明。操作员须核对原目标装备、草稿和 Sent，
再走原投递的受审计处理流程；程序不自动恢复保护或放回仓库。

这部分只有外部适配器行为变化，无新迁移、HTTP 字段或运行配置。源码与隔离证据见[状态页](docs/status.md)，
隔离响应不等同于真实游戏协议或生产发送验收。

## 定期三页维护

MoogleMail 的三个邮箱列表为 Inbox、Read Mail、Sent；compose 是独立的可变草稿，不再被算作第三个只读邮箱。
每个已配置世界在 cron 周期内并行执行 Inbox 处理、Read Mail 观察与 Sent/草稿维护，三者各自有界；
Read Mail 的慢响应或失败不延迟 Sent 核对，维护失败不撤销并行入站处理。共享请求只复用同一有效观察窗口，
新发送基线及写后证据必须跨越新鲜屏障。详情 GET 前仍先持久登记原 MID，读历史不创建新的业务命令。

维护的数据库、凭据/session 排队、Sent 列表及详情、compose/discard 共用既有 fetch_timeout，不给每封详情重建整轮预算。
`000060` 持久化 world 内环形尝试位置；最多 200 条候选、20 条通知/Take，仅在单条开始前推进，未处理后缀保留。

- Sent 命中本地仍为 `manual_reconcile` 且无 `external_reference` 的投递时，把 MID、collected、COD 与剩余资产数写入
  `deliveries.result_metadata.mailbox_sweep`，不自动判成功。
- Sent/Inbox 列表把标题截断到 **35 字节**且不追加省略号；多字节截断点会被页面替换成 `U+FFFD`。匹配使用“去除
  `U+FFFD`/省略号后的前缀”，完整标题和 reference 仍必须由详情页再次确认。
- Sent 摘要中 reference 与收件人唯一匹配、且详情金额/资产矩阵一致时，把历史 `write_unknown` 投递绑定回该 MID：
  `external_reference=mooglemail:main:send:<mid>`、`proof=sent_recovered_reference_v1`，并把履约从
  `manual_reconcile` 重新排队为 `enqueued`，让后续收取证据（`CoD Paid`）继续推进；有资金分配时同事务收口为 `spent`。
  恢复要求当前详情仍保留冻结 Credits/COD/装备集合，已领取后的空资产不能反推原金额。绑定不会重发邮件，也不把恢复
  证明冒充实时发送的 `sent_exact_payload_v1`。
- 有冻结发送尝试的记录覆盖全部来源，包括 `mail_intake`、`settlement`、`settlement_recovery_action`、`operations_alert` 和
  `lot_watch`。匹配完整主题、原正文摘要、世界/收件人、原资产矩阵及原实际发送窗口，验证当前凭据身份；不重渲染历史正文。
  多个同内容候选或竞争发送尝试不绑定；单候选详情读取失败也不能忽略后误认另一封。恢复成功使用 `sent_recovered_frozen_v1`，
  并同事务绑定唯一 MID、收口资金与审计。旧无冻结尝试只保留既有安全 reference/无密钥通知规则，不读当前登录 key 猜测旧信。
- Sent 中没有对应通知不能证明当时未发送；零命中、多命中、时间或关联身份不充分都保留 `manual_reconcile`，
  不自动退回发送队列。恢复绑定不重发邮件，不以模板相同或单独的时间窗口替代唯一发送身份。
- `take` 先核对 `000063` 原尝试冻结的 world/MID 与 credential ID/version，再通过共享 Inbox/Read Mail 定位并读取同账号详情。
  两个列表缺席都不是不存在证明；缺失只接受唯一系统框完整的“Mail does not exist”（允许末尾句点），且不得混有详情节点或隐藏正文。
  完整详情的发件人匹配原收件人且装备、Credits、COD 均归零，才可按 `action_recovered_inbox_v1` 收口。
  只有原装备集合和金额精确相同才可重排（上限 2 次）；部分、额外、重复资产或金额漂移均保留人工核对。
  成功及重排同事务复核当前 generation、原 credential ID/version 和候选 fence；旧无绑定或配置 Cookie 无持久身份的未知 Take 不自动恢复。
  自动领取、释放、收口及过期隔离保留已消费次数；只有现有 Admin 显式重试沿用原重置规则。
  升级不推断或回填旧版本已丢失的计数，不改变原 Take 身份及候选 fence 校验。
- compose 非空时，只有收件人、业务 reference、Credits/COD 和精确装备集合同时匹配唯一未证明投递才执行 discard；
  资产总数可能包含 Credits 条目，不能代替装备 ID 集合。匹配不到或多匹配只记录日志，绝不丢弃来源不明的草稿；通知类没有业务 reference，不参与旧草稿匹配。
- discard 前在原投递上持久登记一次性尝试，再在同一凭据 session 重读并逐字段比较原草稿。
  尝试提交未知、排队/读取取消或 POST 结果未知都保留人工核对，不能因下一轮仍看到草稿而自动再丢弃。
- discard 成功后再读 compose 证明为空，并把结果写入 `result_metadata.compose_discard`。
写后明确登录失效同样触发当前 Cookie 的失效处理，但保留 `write_started` 和未知写入边界；
失效不是未发送证明，不能据此换 Cookie 或自动重发。

## 装备读取失败诊断

每个完整装备响应先执行加密 prepare；只有 request、response、parse 或 validate 阶段的读取/解析/身份/snapshot/filter 失败才在
有界 2 秒内 retain，成功读取直接丢弃密文。装备邮件只保存首个失败的非权威 `equipment_diagnostic` sidecar，不进入
`decision_metadata`、command digest、completion proof 或 cursor。

Admin A18 详情可展示装备 ID、1-based 附件位置、`stage`（`request`、`response`、`parse`、`validate`）、闭集 `category`、可选
`detail_code`、真实可用的 HTTP status、`elapsed_millis`（最多 300000）、evidence UUID 和 retention status/error。
`snapshot_invalid` 与 `filter_rejected` 允许原有对应闭集 `detail_code`；
`stage=parse/category=protocol_drift` 另允许装备来源解析的九个闭集原因：
`equipment_name_invalid`、`equipment_quality_unknown`、`equipment_kind_unknown`、
`equipment_category_unknown`、`equipment_category_mismatch`、`equipment_level_invalid`、
`equipment_page_structure_invalid`、`equipment_attributes_invalid`、`equipment_canonical_mismatch`。
它们只描述失败位置，不含原名称、属性、URL、key 或响应正文；历史缺省细节仍有效，其它组合拒绝来源细节码。
Admin 手工来源失败沿用原 HTTP 错误契约，仅在安全日志记录闭集原因；MM 留存使用现有诊断 sidecar，
不改变业务拒收决定或重放路径。网络、超时、截断或超大响应只标记 `body_unavailable`，不伪称取得完整响应。

这些来源原因的数据库许可由追加迁移 `000056_equipment_source_diagnostic_codes` 提供。
按[停机迁移流程](ops/README.md)先应用迁移，再启动同版角色；旧 CHECK 只允许这类诊断的原因为空，
不能仅更新 Worker 二进制。迁移只扩展允许的来源原因，不重写旧收件、账本、游标或证据。
`equipment-contract` 静态门禁比对应用原因枚举、最新 SQL 约束、Admin 解码和五语言说明，已纳入 `just verify`。

管理列表和详情将已知代码译为说明，同时保留原始代码便于关联日志；装备拒收的摘要位于详情顶部，优先显示留存的具体诊断，
没有 sidecar 时使用已有业务 `reason_code`，都缺失时不猜测原因。`auction_equipment_rejected` 只是拒收总码；
`snapshot_invalid / collected_at_future` 表示采集时间晚于用于该次校验的时间，须检查取时顺序及 Worker 时钟，不能据此认定
Cookie 失效或品质不符，更不能关闭时间校验绕过。`filter_rejected` 说明处理当时的规则未通过；`start_price_*` 指向定价指令。
网络/HTTP 异常与登录失效分开说明，证据 `authentication` 也不等于游戏登录失败；未知代码保留原值，旧记录缺少诊断时不能
通过刷新补齐。收件拒绝不代表装备已退回，须核对关联 Return/Send 任务和 MM；修改配置或升级文案不会自动重放旧收件。
当前[邮件处理器](crates/auction-application/src/services/mail/processing/processor/auction.rs)在每件装备读取完成后重新取应用
时钟校验，保留未来时间门禁及命令业务时间，不会因此处理旧任务。

装备诊断上下文的 `external_equipment_id` 与 `attachment_position` 在通用邮件诊断中保持 optional，缺省时省略，以保持既有
v2 AAD 字节逐字兼容，不升级证据格式；装备证据复用 MoogleMail diagnostics 的父密钥、保留期、容量上限和 `mail-evidence
export` CLI，不新增配置字段。留证不占用业务定时器：应用层返回 `MailProcessingAttempt`，保留原始处理结果和可选的
`PendingMailEquipmentDiagnostic` 句柄，只构造而不轮询其中的 `'static` future；`process_with_retry` 只收集每次已结束尝试的
句柄，`process_claim_inner` 在 `timeout(process_remaining, process_with_retry)` 结束后，才于 `item_timeout` 和 batch
`process_remaining` 两层业务定时器之外以 2 秒上限 resolve。只有 terminal `OkRejected` 才挂接 equipment sidecar，中间
`Err` 也留证/写日志但保留原 retryability 分类；实际 retention elapsed 会延长后续 batch deadline，避免本封留证占用下一封
业务预算。日志出口是 Worker 始终注入的同步 callback，integration 不新增 tracing 依赖，raw recorder 只在开关启用且同一
实例满足条件时注入。HTTP body timeout 仍只保留 `body_unavailable` 与最后实际观测到的 header status，session expired 在
凭据耗尽或失效写回失败时保留最后已取得的响应 body、status 和 elapsed。

## 历史资产处置核对

升级按[停机/迁移流程](ops/README.md)执行。可恢复的本地接受决定与未知远端资产须分开核对，不能修改旧 payload、命令 ID 或
账本解除阻断。

- `legacy_return_requires_review`：旧流拍 Return 已冻结或有过执行，保持隔离。核对原投稿、逐件装备库存和 Sent/原邮件
  动作证据，不能直接将它改为 Send；迁移 `000038` 只自动转换从未开始且无关联 delivery 的记录。
- `mail_disposition_conflict`：同一邮件存在已接受投稿与 Return，或同时存在 Take/Return 意图。后台保留原负载，在发送前转
  人工核对；确认后只能将该任务标记失败，这不会消除资产矛盾，也不会开放重试或伪造发送成功。后续处置须根据权威投稿、
  库存及远端回执另行确认，保留追加式审计。
- 修复前已归档的未知标题 Credits 邮件：新逻辑不会重写既有完成事实或自动越过原游标重放。应按 world、邮箱、外部 MID
  对照原入站记录及当前远端资产盘点；没有证据时不能推定款项已领取或退回。

这些核对项不代表已经迁移、盘点资产或完成外部写入；具体证据见[验证状态](docs/status.md)。
