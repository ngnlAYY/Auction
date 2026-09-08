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
