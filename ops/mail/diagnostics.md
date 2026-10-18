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

## 主动 ComposeSend 的 Sent 证明

主动 ComposeSend 固定四步验证：①发送前读取 compose 并证明为空表单；②对 Sent 做覆盖所有 MID 的完整有界
baseline；③发送后执行独立有界 Sent scan 并要求收到新 MID 且字段精确匹配；④再次读取 compose 并证明回到空表单。
任一步失败或页面 `mail_state` 与待领取资产矩阵不一致都标记结果未知。成功条件是
新增且唯一的真实 MID，并同时精确匹配 world、recipient、subject、canonical body digest、Credits、COD 和装备 ID；0 资产
通知也要求 0 资产 Sent 证明。资产不符、秒领取、无匹配、多匹配、未扫完或超时均标记结果未知并进入 `manual_reconcile`，
不自动重寄；成功页、空 compose、本地 UUID 都不能替代证明。该规则覆盖静态通知、注册/重绑 key、运维告警及所有结算/返还
金钱和装备。

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

## 定期三页维护

MoogleMail Worker 每个 cron 周期（默认 5 分钟，仅 main）在收件轮询前额外读取三页：Sent 全量摘要、按需读取匹配 MID 的
详情，以及 compose。该维护只追加证据，不改变投递状态：

- Sent 命中本地仍为 `manual_reconcile` 且无 `external_reference` 的投递时，把 MID、collected、COD 与剩余资产数写入
  `deliveries.result_metadata.mailbox_sweep`，不自动判成功。
- Sent 摘要中 reference 与收件人唯一匹配、且详情金额/资产矩阵一致时，把历史 `write_unknown` 投递绑定回该 MID：
  `external_reference=mooglemail:main:send:<mid>`、`proof=sent_recovered_reference_v1`，并把履约从
  `manual_reconcile` 重新排队为 `enqueued`，让收取证据（`CoD Paid`）继续推进。恢复证明只用于升级前的未知投递，
  不改变实时发送仍必须使用 `sent_exact_payload_v1` 的规则；绑定不会重发邮件。
- compose 非空时，只有收件人、业务 reference 和 COD 同时精确匹配唯一一条未证明投递才执行 discard；匹配不到或匹配到多条
  只记录日志，绝不丢弃来源不明的草稿。
- discard 成功后再读 compose 证明为空，并把结果写入 `result_metadata.compose_discard`。

## 装备读取失败诊断

每个完整装备响应先执行加密 prepare；只有 request、response、parse 或 validate 阶段的读取/解析/身份/snapshot/filter 失败才在
有界 2 秒内 retain，成功读取直接丢弃密文。装备邮件只保存首个失败的非权威 `equipment_diagnostic` sidecar，不进入
`decision_metadata`、command digest、completion proof 或 cursor。

Admin A18 详情可展示装备 ID、1-based 附件位置、`stage`（`request`、`response`、`parse`、`validate`）、闭集 `category`、可选
`detail_code`、真实可用的 HTTP status、`elapsed_millis`（最多 300000）、evidence UUID 和 retention status/error。
`snapshot_invalid` 与 `filter_rejected` 才允许对应闭集 `detail_code`，其它 category 不带细节原文；网络、超时、截断或超大
响应只标记 `body_unavailable`，不伪称取得完整响应。

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
