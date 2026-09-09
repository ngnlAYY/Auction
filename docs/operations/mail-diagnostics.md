# MoogleMail 异常诊断与原始数据留证

MoogleMail 的业务拒绝、解析异常、HTTP 异常和处理超时需要同时保留可检索的安全日志，以及在确有异常时保留原始数据，便于确认邮件正文到底被怎样解析。普通成功邮件不留证；业务结果、数据库、DTO 和普通日志仍只保存安全字段，不能写入正文、Cookie、请求表单、token 或装备 key 明文。

## 普通日志

Worker 日志按 `world_id`、`mailbox_key`、`external_mail_id`、阶段、操作和结果分类关联一封邮件。业务定价拒绝继续使用原有 `reason_code`；定价诊断只增加有界的 `pricing_diagnostic.detail_code`、`body_bytes`、`token_count`、`field_index` 和 `attachment_count`。这些字段用于判断解析分支，属于确定性的业务诊断字段，会随决定进入 `decision_metadata`；随机 `evidence_id` 和诊断留证回执不能进入幂等比较字段。

异常日志会给出 `evidence_id`（UUID）、`evidence_status`（`retained`、`body_unavailable` 或 `save_failed`）和静态错误分类。`body_unavailable` 只表示留证当时没有取得响应正文或没有取得完整响应，例如 HTTP 超时；磁盘容量淘汰不会改写当时的状态。淘汰后的 UUID 导出返回 `not_found`。留证失败只产生 warning，不改变邮件退回、投递、重试、游标或幂等结果。

配置关闭或加密准备失败时分别记录 `disabled`、`prepare_failed`。租约取消或外层超时中断留证等待时记录 `completion_unconfirmed`；已经开始的本地写入可能仍在完成，不能把该状态当作“已保存”或“确定没有文件”。

## 保存内容与边界

留证分为两类：

- HTTP 异常保存实际取得的原始响应字节；若远端返回了 HTTP status 则一并保存，否则状态为 unknown，响应正文上限为 8 MiB。解析异常只在已成功读取的正文上运行，解析层不再持有 HTTP status，因此不会伪造 `200`；其证据状态为 unknown。
- 业务拒绝、业务失败和处理超时保存实际提取的 JSON：`body`、`subject`、`sender`、附件外部 ID/key、`credits`、`cod`、`returnable`、`claimable`、`received_at` 和 `content_digest`。`body` 保留 Unicode 空白，以便复核类似 `<equipment-url> start 50k` 的输入。

请求 Cookie、请求表单 payload 和未实际取得的响应不进入证据。响应 HTML 中的 token 整体加密；原始数据只存在独立证据文件，不跨批次保存 HTML，也不保存正常邮件。当前邮件若在新功能启用前已处理，系统没有历史原文，不能由新功能恢复。

证据由父数据密钥通过固定独立 HMAC 域派生诊断子密钥后使用 AES-256-GCM 加密；认证安全 public header 包含 `key_source`、`key_id`、`outcome` 和 `context`，正文保持加密，并通过完整 AAD 认证元数据。优先使用 `credential_data_key`，缺少时使用 `equipment_data_key`；这只是诊断留证的明确派生例外，不直接复用凭据 AES 密钥。证据目录为 `0700`，文件为 `0600`，保留 1—365 天，最多 1024 个文件，并在写入时按总字节上限和保留期清理旧文件；不宣称存在独立定时清理，也不涉及自动重放。

该能力属于 PostgreSQL 完整档的 Worker 运维能力，不新增数据库表或迁移；SQLite Lite 不新增该能力。诊断文件是本地运维证据，不属于业务事实、入站幂等账本或持久化游标。

## 配置与离线导出

Worker 主配置可使用以下区块，目录相对 Worker TOML 解析。父目录必须已存在，路径不能经过符号链接；现有证据目录必须归运行账号所有且权限为 `0700`。当前私有文件适配器只在 Unix 上启用，其他平台明确返回 `unsupported_platform`，不会降级保存。

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

可用 `--key-source credential` 或 `--key-source equipment` 指定历史父密钥来源；省略时优先 credential，缺失时使用 equipment。输出路径使用 `create_new` 和 `0600`，已存在文件不会覆盖；命令不把正文打印到 stdout。证据已被容量或保留期清理时按 UUID 导出会返回 `not_found`。历史导出必须继续保留对应旧父密钥备份，当前实现不会自动轮换或迁移诊断密钥。

## Admin 审查展示投影

完整 `MailMessage` 首次写入 `inbound_mail` 时同步生成敏感审查投影。列表只返回可空的 `sender`、`subject`
scalar 和 `has_message_display`；详情才返回 `message_display`。snapshot 固定 schema/redaction version，
正文最多 64 KiB、JSON 最多 128 KiB、附件外部 ID 最多 1000 条；Credits/COD 保留为非负数。装备 key、URL key
片段、认证邮件正文和认证相关敏感内容均隐藏。该投影属于 PostgreSQL 完整档的非权威展示数据，不参与入站幂等
或 cursor，普通 replay 不会补写历史 NULL；SQLite Lite 不新增列或能力。

历史回填只接受认证的 `parsed_message` 诊断证据，并要求 world、mailbox、external mail ID 和 content digest
精确匹配，再以 CAS 和追加审计写入。相同幂等身份但内容不同必须冲突；没有匹配证据的历史邮件显示未留存。证据
保存期限和本地权限沿用本页配置，回填不会把证据变成业务权威事实。

`auctionctl mail-evidence inspect-sent <obligation-id> --output <path>` 是 PostgreSQL 完整档的只读 Sent 诊断，
范围仅限 `withdrawn_return` 单件装备、零 Credits、`COD=0` 的义务。命令按页数和总 deadline 读取，要求唯一且
精确匹配 world、recipient、canonical subject 和装备 ID，并把结果标记为 `time_unverified`；它不是通用 payment
核对，也不是已送达证明。`collected` 只表示结果尚未取得充分证明，不会自动 reconcile、send，也不领取或修改
delivery、fulfillment、lease 或远端邮件；输出文件必须是新的私有路径。

## 主动 ComposeSend 的 Sent 证明

所有主动 ComposeSend 都必须先对 Sent 做覆盖所有 MID 的完整有界 baseline，再发送，随后执行独立有界 Sent scan。
成功条件是新增且唯一的真实 MID，并同时精确匹配 world、recipient、subject、canonical body digest、Credits、
COD 和装备 ID；0 资产通知也要求 0 资产 Sent 证明。资产不符、秒领取、无匹配、多匹配、未扫完或超时均标记
结果未知并进入 `manual_reconcile`，不自动重寄；成功页、空 compose、本地 UUID 都不能替代证明。

该规则覆盖静态通知、注册/重绑 key、运维告警及所有结算/返还金钱和装备。外层 delivery timeout 默认 180 秒、
sending grace 默认 300 秒；启用 MoogleMail 时 timeout 必须大于两倍 fetch budget，读取复用既有
`maximum_pages` 和 `fetch_timeout`，不新增配置字段。成功 metadata 为 `sent_verified=true`、
`external_reference_proven=true`、`proof=sent_exact_payload_v1`，真实引用为 `mooglemail:world:send:mid`。
`sender/proof.rs` 的 `parserSentContentProof` 负责内容证明，storage 成功写回前执行 defense gate。
Return/Take 使用独立 action proof，不属于主动 ComposeSend。

Admin 人工 reconcile 不能把 `payload.operation=send` 与裸 `external_reference` 填成 `succeeded`；后端会按既有
invalid 条件拒绝，Admin Web 隐藏该成功选项。`retry` 和 `failed` 仍需显式管理员命令与审计，Return/Take 保留
独立人工核对路径。该防御不增加 HTTP 字段或错误类别。

收件详情的“关联邮件任务”表仅用于查看与手动重发：`operation` 为 `Send`、`Return` 或 `Take`，对应按钮分别为
“重新发送”“重新退回”“重新领取”。只有 `failed` 或 `manual_reconcile` 可操作；`pending`、`claimed`、`sending`、
`succeeded`、`prepared` 均不可重试。确认后复用既有 delivery retry/reconcile（`action=retry`），携带稳定
`command_id`、CSRF 和 `expected_fencing_version`；成功文案只表示“重新排队”。结果未知时提示先核对 MM，不在历史
收件业务决定上重放，也不执行当前 MM 中已有任务。该表不是新的 HTTP 路由、迁移或配置能力。

## 装备读取失败诊断

每个完整装备响应先执行加密 prepare；只有 request、response、parse 或 validate 阶段的读取/解析/身份/
snapshot/filter 失败才在有界 2 秒内 retain，成功读取直接丢弃密文。装备邮件只保存首个失败的非权威
`equipment_diagnostic` sidecar，不进入 `decision_metadata`、command digest、completion proof 或 cursor。

Admin A18 详情可展示装备 ID、1-based 附件位置、`stage`（`request`、`response`、`parse`、`validate`）、闭集
`category`、可选 `detail_code`、真实可用的 HTTP status、`elapsed_millis`（最多 300000）、evidence UUID 和
retention status/error。`snapshot_invalid` 与 `filter_rejected` 才允许对应闭集 `detail_code`；其它 category 不带
细节原文。网络、超时、截断或超大响应只标记 `body_unavailable`，不伪称取得完整响应。

安全日志始终包含 world/mailbox/external mail ID/equipment ID/附件位置等关联字段，即使留证 disabled、
prepare_failed、save_failed 或 completion_unconfirmed。状态需区分 `disabled`、`prepare_failed`、`save_failed`、
`completion_unconfirmed`、`retained` 和 `body_unavailable`；evidence UUID 不保证文件永久存在，可能因保留期或
容量清理后导出为 `not_found`。旧邮件若没有底层失败 trace，不补写失败原因，也不重放旧邮件。
装备诊断上下文的 `external_equipment_id` 与 `attachment_position` 在通用邮件诊断中保持 optional；缺省时省略，
以保持既有 v2 AAD 字节逐字兼容，不升级证据格式。
装备证据复用现有 MoogleMail diagnostics 的父密钥、保留期、容量上限和 `mail-evidence export` CLI，不新增配置字段。

留证不占用业务定时器：应用层返回 `MailProcessingAttempt`，其中保留原始处理结果和可选的
`pendingEquipmentDiagnostic`，只构造句柄，不轮询 static future。`process_with_retry` 只收集每次已结束尝试的句柄；
`process_claim_inner` 在 `timeout(process_remaining, process_with_retry)` 结束后，才于 `item_timeout` 和 batch
`process_remaining` 两层业务定时器之外，以 2 秒上限 resolve。只有 terminal `OkRejected` 才挂接 equipment sidecar；
中间 `Err` 也留证/写日志，但保留原 retryability 分类。实际 retention elapsed 会延长后续 batch deadline，避免本封
留证占用下一封业务预算。日志由 Worker 始终注入的同步 callback 出口提供；integration 不新增 tracing 依赖，raw
recorder 只在开关启用且同一实例满足条件时注入。

HTTP body timeout 仍只保留 `body_unavailable` 与最后实际观测到的 header status，不把 partial body 伪称完整响应；
session expired 在凭据耗尽或失效写回失败时保留最后已取得的响应 body、status 和 elapsed。
