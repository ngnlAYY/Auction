# 配置参考

[docs/README.md](../../docs/README.md) · [ops/README.md](../README.md) · [ops/config/examples](examples) · [crates/auction-config/README.md](../../crates/auction-config/README.md)

所有运行配置都是 TOML。字段的权威说明是 [ops/config/examples](examples) 中每个字段上方的中文注释（标明“可选/必填”、默认值与范围）；本页说明文件结构、加载规则与各区块的用途。

## 文件结构

```text
/etc/auction/
├── public.toml          Public 角色文件
├── admin.toml           Admin 角色文件
├── worker.toml          Worker 角色文件
├── common.toml          三角色共用的非敏感配置
├── secrets.toml         敏感配置（0600）
└── runtime/
    └── admin.toml       runtime 配置（Admin 后台编辑，0640）
```

| 文件 | 拥有 |
| --- | --- |
| 角色文件 | `common_file`、`secrets_file`（必填，相对角色文件所在目录解析）；可选 `runtime_config_file`；Admin 另有 `worker_config_file`；listener、tracing、HTTP 安全、会话（Public/Admin）、realtime（Public）、`[worker.*]`（Worker） |
| `common.toml` | `log_filter`、`[metrics]`、`[readiness]`、`[auction.*]`、CLI 专用 `[backup]` |
| `secrets.toml` | 数据库、外部 Cookie、Admin 登录凭据、四组数据密钥（含旧密钥环）、OTLP 导出头、R2 令牌与可选打卡/心跳/Webhook 地址 |
| `runtime/admin.toml` | 可由 Admin 后台修改的运行参数（见下文） |

### 加载规则

- 角色文件发现：Public/Worker 先读 `AUCTION_CONFIG_FILE`，未设置时尝试 `ops/config/<角色>.toml`，不存在则用内置默认配置；Admin 另尝试 `auction-config/admin.toml`，仍不存在则进入首次设置。`auction-server` 与 `auction-cli` 用 `--config-dir`，`auction-server` 检测到 `AUCTION_CONFIG_FILE` 会拒绝启动。
- 除 `AUCTION_CONFIG_FILE` 外不存在环境变量覆盖；数据库运行时开关可覆盖 TOML 基线（见下文）。
- 已移除的字段不会静默忽略：加载器解析旧键后在校验阶段以可行动说明拒绝（例如 `[worker.delivery] concurrency`）。角色文件只接受该角色实际读取的区块：Public 拒绝 `[worker]`；Admin 拒绝 `[realtime]`；Worker 拒绝 `[listener]`、`[http_security]`、`[realtime]`、`[session]`。错误给出角色、区块与 owner 文档位置，不回显配置值。
- 解析与校验错误在启动前失败，不回显秘密；文件大小上限 4 MiB（维护凭据 64 KiB）。三个角色必须引用同一 `common_file`、同一 `runtime_config_file`（或全部省略），且数据库目标相同；`auction-cli config check` 联合检查。

## 敏感文件（`secrets.toml`）

| 组 | 字段与要求 |
| --- | --- |
| 数据库 | `[database] host/port/username/password/name`（或兼容的 `database_url`，二者互斥）；`username` 必须是迁移 081 授权的 `auction_runtime`，不得使用 owner 或维护账号 |
| Admin 登录 | `admin_actor`、`admin_secret` 成对；既有 secret 16–4096 字节可加载，首次设置要求 ≥32 字节 |
| 数据密钥 | `equipment/user_login/credential/mail_diagnostics` 四组 `*_data_key_id` 与 `*_data_key`（32 字节标准 Base64，`openssl rand -base64 32`），标识与材料成对、各组不复用；`*_previous_data_keys` 为 `id`/`key`/`retired_at` 数组，保留期 `retired_at + 12 个月`；三角色必须一致 |
| 外部 Cookie | `mooglemail_cookie`、`ehentai_cookie`（不使用数据库凭据时） |
| OTLP | `public/admin/worker_otlp_headers` |
| 打卡与告警 | `backup_heartbeat_success_url`/`failure_url`（各自可选）、`worker_heartbeat_url`（默认空即关闭）、`operations_alert_webhook_url`（webhook 启用时必填）；只接受无 userinfo/fragment 的 HTTPS URL，`config check` 校验且不回显 |
| R2 | `backup_r2_access_key_id`/`secret_access_key` 成对、非空、无控制字符；异机启用时必填 |

密钥缺失时对应能力不可用（接口返回能力不可用，Worker 相关功能拒绝启用）。密钥环允许多 key ID 并存解密，新写入只用当前 key；轮换、离线重加密与保留期见[ops/keys/README.md](../keys/README.md)。文件须仅 owner 可访问（0600）；`admin_actor`、密钥、Cookie 与 OTLP headers 等根字段须写在 `[database]` 表之前。高权限凭据与运行账号分成两份独立文件，常驻角色只引用 `secrets.toml`。

## common.toml

| 区块 | 用途 |
| --- | --- |
| `log_filter` | tracing 过滤器，默认 `info,<服务名>=debug`；语法是逗号分隔的 `级别` 或 `target=级别`（级别为 `trace`/`debug`/`info`/`warn`/`error`/`off`，target 按前缀匹配、最长者生效），不支持按 span 或字段过滤，空值、裸 target 与含空白的写法在 `config check` 与启动时被拒绝；追加 `auction_observability::log_dedup=off` 可重启后关闭重复错误去重 |
| `[metrics]` | 进程指标开关、转发拒绝与可选附加出口（见下） |
| `[readiness]` | 审计失败模式、迁移兼容模式与 HTTP 就绪缓存，见[就绪策略](#就绪策略) |
| `[backup]`、`[backup.offsite]`、`[backup.scope]` | CLI 专用定时备份、异机副本与数据库之外的范围，见[定时备份策略](#定时备份策略) |
| `[database_roles]` | `owner` 是迁移 owner 账号名（生产严格预检必填），运行账号固定 `auction_runtime` 不可配置；两者相同则拒绝 |
| `[auction.schedule]` · `[auction.schedule.early_bird]` | 周排期（时区、预览/开拍/结束时间、参与世界）；赛季早鸟窗口与赛季前场次 |
| `[auction.economics]` · `[auction.settlement]` · `[auction.settlement.recovery]` | 新场次经济规则；结算策略（`hold_curve_v2` 或 `legacy_zero_v0`、补偿池、isekai 载体、曲线锚点）；恢复策略（v1 7 天、v2 24 小时，提醒 1 次，COD 退回上限可配） |

`metrics.enabled` 默认 false。Public/Admin 开启后业务 listener 必须是 loopback TCP 或 Unix socket；Worker control 按原 peer 授权提供指标。以下部署层可选字段允许 Prometheus 直接抓本机出口：

| `[metrics]` 字段 | 默认与作用 |
| --- | --- |
| `public/admin/worker_loopback_listen_address` | 省略关闭；各角色附加指标地址（示例 19100/19101/19102） |
| `reject_forwarded_requests` | 默认 true；业务指标路由及新出口拒绝转发的 TOML 基线，在线开关只允许收紧 |
| `sli_snapshot_enabled` | 默认 false；Worker 六十秒只读汇总的 TOML 基线 |

有效 `enabled = true` 且填写本角色地址时才监听；地址只接受数值 `127.0.0.1` 与非零端口，拒绝主机名、通配、其他 IP，三个地址不能重复、不能与业务 listener 冲突（Admin 按 runtime 生效后核对）。Admin runtime 的 `[metrics]` **只覆盖 Admin 的 `enabled`**；拒绝策略基线与地址始终来自 common。放宽拒绝或改地址/`enabled` 须重启。附加出口只提供 `/metrics`，绑定失败只降级采集；转发拒绝按头存在性覆盖所有方法并优先于 request-id 400，契约见[docs/contracts/routes.md#健康与指标publicadmin](../../docs/contracts/routes.md#健康与指标publicadmin)。经济、结算与恢复策略在创建场次时冻结，修改模板只影响之后的场次。

### SLI 快照

仅 Worker 消费，须同时启用有效 metrics。启动时把 `sli_snapshot_enabled` 注入运行时开关，数据库 bool 开关 `metrics_sli_snapshot_enabled` 可在线覆盖，clear/到期回落 TOML 基线；改基线须重启，回退旧程序须移除未知字段。汇总最多借一个空闲连接、总预算两秒，失败保留旧指标及原年龄；未实现的历史流程标为待索引。见[ops/README.md#sli-基线与容量观察](../README.md#sli-基线与容量观察)。

### 就绪策略

`common.toml [readiness]` 是三角色共同引用的启动快照，全部字段可省略，不由 Admin runtime 覆盖；改文件基线须重启，数据库开关覆盖后续行为无需重启。枚举非法拒绝加载且不回显原文。

| 字段 | 默认值 | 语义 |
| --- | --- | --- |
| `audit_failure_mode` | `"shadow"` | `legacy`：任一审计失败即粘滞未就绪；`shadow`：沿用旧就绪与全部失败心跳计数并比较分类门禁；`classified`：只由 permanent + unknown 粘滞阻断 |
| `migration_compatibility_mode` | `"shadow"` | `strict`：完整历史必须恰好匹配；`shadow`：实际仍严格，同一快照计算兼容判定并记录差异；`compatible`：显式允许未来后缀均已登记兼容的数据库 |
| `cache_enabled` | `true` | Public/Admin 最终就绪布尔值正负均缓存 1 秒、并发共享一次 750ms 检查；`false` 逐次执行；Worker control 不使用该缓存 |

分类口径与粘滞判定由[crates/auction-storage/README.md#审计失败与就绪](../../crates/auction-storage/README.md#审计失败与就绪)拥有，迁移判定由[docs/contracts/database.md#迁移兼容登记与就绪](../../docs/contracts/database.md#迁移兼容登记与就绪)拥有。默认基线 shadow：差异只应出现在 transient，没有样本不能解释为通过，由用户审阅后才切换 classified；迁移模式平时保持 shadow，换回旧程序前须显式设 `compatible` 并按[ops/deploy/RECOVERY.md#程序回退](../deploy/RECOVERY.md#程序回退)核对。观察步骤见[ops/README.md#审计就绪影子观察](../README.md#审计就绪影子观察)。

### 定时备份策略

`[backup]` 全部字段可省略，只有 `backup scheduled` 与 `config check` 消费；不进 Admin runtime，启用与回退即用户启停 timer。

| 字段 | 默认值 | 范围 |
| --- | --- | --- |
| `local_retention_count` | `7` | 1–365，本地已完成定时备份份数 |
| `min_free_bytes` / `min_free_percent` | 1 GiB / 10 | 64 MiB–1 TiB；1–50，与字节水位同时满足 |
| `timeout_seconds` | `7200` | 60–7200，自本次启动起的任务预算 |

打卡客户端一次 HTTPS GET（连接三秒、请求五秒，不重试、不用环境代理/重定向/Cookie），失败不改变备份结果，终态回执只保存 `heartbeat_configured`；外部打卡按决策暂缓启用。细节见[ops/deploy/RECOVERY.md#定时备份](../deploy/RECOVERY.md#定时备份)。

#### 异机副本

`[backup.offsite]` 默认关闭，只由 CLI 消费；`enabled = false` 在下次 scheduled 生效，暂停加密、上传与旧待上传项重试，不影响本地备份。启用前必须完成[ops/deploy/RECOVERY.md#异机副本前置准备](../deploy/RECOVERY.md#异机副本前置准备)，特别是 bucket lock，CLI 不代替用户检查控制台策略。

| 字段 | 默认值 | 校验 |
| --- | --- | --- |
| `enabled` | `false` | true 时 `age_recipient`、`endpoint`、`bucket` 与成对 R2 凭据必填 |
| `age_recipient` | `""` | age 原生 X25519 公钥（62 字节，`age1` 前缀）；不提供私钥或口令字段 |
| `endpoint` / `bucket` / `prefix` | `""` | HTTPS 根地址无 userinfo/query/fragment；bucket 3–63 字节小写字母数字连字符；prefix ≤512 字节、无首尾斜杠、空段或 `..` |

`auction-cli wal upload` 复用同一目的地、`prefix` 与凭据：WAL 段为 `<prefix>/wal/<段名>.zst.age`，基础备份为 `<prefix>/base/<备份 ID>/`；WAL 归档与基础备份命令不读配置，用 `--recipient` 传入同一公钥，改公钥须同步两处。凭据由 `SecretString` 保存、只经显式 getter 交给适配器；`config check`/`config show` 只输出 enabled 与是否配置，绝不输出值。轮换令牌不改变待上传身份；换公钥或目的地后哈希不匹配的旧项保留并记 `pending_destination_changed`。见[ops/deploy/RECOVERY.md#wal-归档与时间点恢复](../deploy/RECOVERY.md#wal-归档与时间点恢复)。

#### 备份范围（数据库之外）

`[backup.scope]` 默认关闭；启用后 `backup create` 与 `backup scheduled` 在数据库 archive 之外把**本轮实际加载的配置文件**、Worker 邮件诊断证据与可选部署回执目录收集为密文包（明文临时文件打包后立即加密删除，sidecar `auction-backup-scope.v1` 只记录相对路径、字节数、SHA-256 与摘要）。字段：`enabled`（true 时须有 `age_recipient` 且至少一个来源）、`runtime_config`（默认 true）、`mail_diagnostics`（默认 true）、`deployment_receipts_dir`（绝对路径，建议 `<state-dir>/receipts`）、`max_files`（默认 4096，1–8192）、`maximum_total_bytes`（默认 256 MiB，按压缩前明文计）。`backup verify` 校验 sidecar 配对与摘要；`backup restore` 只恢复数据库，非数据库材料用 `backup scope-extract` 解密到空的新目录（0600、不覆盖），解出的配置与回执含秘密，只能放离线或隔离环境。

## 角色文件

### Public / Admin

| 区块 | 用途 |
| --- | --- |
| `[listener]` | `mode = "tcp"`（`address`，默认 Public 8090、Admin 8091）或 `"unix"`（运行目录、socket 路径、组）；Unix 运行目录须归 root 或运行账号、禁止 group/other 写入、无符号链接 |
| `[http_security]` | origin allowlist（≤16 个精确 origin）、登录/写请求限流、匿名读限流与并发（仅 Public，IPv6 按 /64）、`cookie_secure`（生产必须 true）、请求超时、`trusted_proxy_cidrs`（≤64）、`trusted_unix_proxy` |
| `[session]` | Public `max_public_sessions`（默认 10，满额注销最久未活动会话）；Admin `idle_ttl_seconds`（900）与 `absolute_ttl_seconds`（28800）。Admin 角色文件写 `max_public_sessions` 会被拒绝 |
| `[tracing]` | OTLP/HTTP 导出（开关、endpoint、批量与超时、队列容量、采样百万分率） |
| `[realtime]`（Public） | `consumer_key`（每实例唯一）、轮询间隔、每页流数与事件数、每连接缓冲 |

Admin 配置 runtime 时，listener、HTTP 安全、会话、`metrics.enabled` 与 tracing 以 runtime 为准；指标拒绝策略与地址始终来自 common。

### Worker

| 区块 | 用途 |
| --- | --- |
| `[worker]` | 拍卖主循环：并发、轮询、批次上限、抖动、单项超时、恢复重试间隔 |
| `[worker.delivery]` · `[worker.fulfillment]` · `[worker.recovery]` | 投递 outbox（并发不可配置，MoogleMail 始终 1）；履约；恢复（低余额阈值、健康刷新与证据过期） |
| `[worker.control]` | Worker control：开关、socket 组、`allowed_peer_uids`/`gids`（至少一项）、运行目录与路径 |
| `[worker.forum]` | 论坛同步：开关、凭据 scope、origin、板块、轮询、页数上限、间隔与租约 |
| `[worker.mooglemail]` | 邮件：开关、邮箱与世界、默认起拍价、请求边界、收件与维护 cron、观察间隔、每日完整校验、白名单、回信预算、`keyenv_file`（本地联调） |
| `[worker.mooglemail.send_intervals]` · `[send_classes]` · `[diagnostics]` · `[equipment_filter]` · `[mail_templates]` | 各类出站邮件最小间隔；六类通知开关（默认全 true，runtime `mail` 组接管后以 runtime 为准）；诊断归档；投稿过滤；正文模板（渲染时冻结） |
| `[worker.operations_alert]` | 告警：`scan_enabled`（true）、`notify_enabled`（false）、`webhook_enabled`（false）、收件人与世界、阈值；旧 `enabled` 只是兼容别名 |

各循环预算按其工作方式分别校验（fulfillment/recovery 单项超时 < 租约；delivery 依靠续租可跨初始租约；MoogleMail 另校验读取/处理/整批预算），不能统一按“请求超时 < 租约”配置。启用外部功能的依赖：MoogleMail 需凭据来源与 `user_login_data_key`；论坛需 `ehentai_cookie` 或 `credential_data_key`；告警扫描只需数据库，通知/Webhook 各有开关与地址要求。webhook 模板须产生 JSON 对象（支持 `{kind}/{scope}/{source}/{count}/{age_seconds}`），创建时冻结并携带冻结的 `Idempotency-Key`。

#### MoogleMail 收件与维护

以下字段只属于 `worker.toml [worker.mooglemail]`，不由 Admin 参数界面接管：`poll_isolation_enabled`（默认 true，在线开关基线；false 恢复每轮三分支 join）、`maintenance_schedule`（省略时跟随生效的收件 schedule）、`read_mail_observation_interval_seconds`（默认 3600，60–86400，首次观察延后一个间隔）。两个世界同时启用时，启动时按收件日程首两个未来触发点计算相位 `min(150 秒, 间隔/2)`，main 零相位、isekai 用该相位；收件与维护整条日程平移后叠加 jitter，手动触发立即唤醒收件、不受相位影响。在线切换用 `auction-cli switch set mooglemail_poll_isolation_enabled false`，下一轮调度或维护门复读后生效，在途轮次按开始模式完整结束，不重写 `mail_poll_state.next_poll_at`；显式维护日程与观察间隔的修改仍需重启。详见本页字段注释与 `ops/config/examples/worker.toml`。

#### MoogleMail 发送前基线

`[worker.mooglemail] sent_baseline_mode` 可选，默认 `"shadow"`，只接受 `full`/`shadow`/`watermark`；只属于 Worker TOML，不进 runtime 或在线开关，修改须受控重启。

| 值 | 行为 |
| --- | --- |
| `full` | 完整 Sent 排除集及十分钟缓存，保留旧路径 |
| `shadow` | 旧路径唯一裁决，只读比较水位新增 MID 集合与列表候选集合，不增加 POST |
| `watermark` | 发送前 fresh 读完最新分钟组取最大 MID 与分钟；发送后核对锚点、读完边界分钟，按 `MID > W` 取候选再走完整详情与 compose 证明 |

边界随 prepared 句柄一次消费，不新增持久水位表。Empty 需空列表与末页证明；仅水位路径要求 MID 为无前导零的 `u128` 受检十进制且同账号同世界单调不复用。写前边界无效回退完整扫描；写后锚点消失则完整枚举并保留原边界；MID/时间前提失效且无写前完整集合时保持 `write_unknown`。回退受 `maximum_pages`（1–100）、原截止、限速与资源限制，未知结果绝不重发。默认 shadow 不能解除 D3，须在旧路径触顶前观察并切换；观察与切换见[ops/README.md#sent-发送基线影子观察](../README.md#sent-发送基线影子观察)。Read Mail 与 Sent 常规维护共用持久水位前缀扫描与 `mail_scan_daily_full_interval_seconds`（默认 86400）完整校验，Inbox 仍以 `mail_poll_state` 游标为权威。

## runtime 配置（`runtime/admin.toml`）

runtime 文件让运营人员在 Admin 页面修改部分参数。Admin 是唯一写者：候选先完整校验，再以命令账本 + 原子替换写入（临时文件、rename、目录同步、独立锁）；文件 0640、父目录禁止他人写入、归 Admin 运行账号所有。**热读取字段在 Worker 下一读取周期生效，其余在重启相关角色后生效**，都不改写已有场次与任务。

| 区块 | 用途 | 生效 |
| --- | --- | --- |
| `allow_manual_early_open/settlement` | 允许 Admin 提前开拍/结算 | 重启 Admin |
| `bot_players` | Bot 角色名列表 | 重启 Public、Worker |
| `[worlds.main]` · `[worlds.isekai]` | 各世界收件开关与发件人白名单 | 重启 Worker |
| `[equipment_filter]` | 投稿装备过滤 | 重启 Worker |
| `[equipment_colors]` | 论坛装备标色与公开站“稀有/融合”分组 | 重启三角色 |
| `[mail_templates]` | 出站邮件正文（创建提醒时渲染冻结） | 重启 Worker（Public/Admin 创建的提醒需同时重启对应进程） |
| `[[dashboard_mailboxes]]` | 仪表盘核对的世界与邮箱 | 重启 Admin |
| `[equipment_source]` | Admin 录入策略（装备页面读取由 Worker 执行） | 重启 Admin |
| `[listener]` · `[metrics]` · `[security]` · `[session]` · `[tracing]` · `[worker_control]` | Admin 自身监听、指标 enabled、HTTP 安全、会话、tracing 与 control 路径 | 重启 Admin |
| `[operations.*]` | 接管部署参数（见下） | 见下 |

### operations 接管

`[operations.<组>]` 的 `managed = true` 时覆盖部署配置，`false`（默认）沿用 common 与角色文件。组：`economics`、`schedule`（重启 Admin 与 Worker）；`forum`、`alerts`、`worker`（含 delivery/fulfillment/recovery）、`mail`（含 send_intervals/send_classes；不含全局开关、endpoint、凭据、邮箱身份与诊断路径）。即时生效字段见下表，其余重启 Worker。启用接管时 `config check` 核对 Admin 与 Worker 的有效模板与排期一致。

### Worker 热读取字段

Worker 每 30 秒检查 runtime 文件长度与修改时间；变化时先完整校验再原子替换节奏与循环参数，失败保留旧值并告警。热读取只改变节奏、预算与反压，不决定金额、资格、幂等、租约、审计或业务状态。

| 组 | 即时生效字段 |
| --- | --- |
| `mail` | `send_intervals` 全部间隔、`send_classes` 六类开关、`batch_size`、`jitter_seconds`、fetch/process/item/batch 超时与重试、`account_reply_window_seconds`、`explanation_mail_window_seconds`、`explanation_mail_limit` |
| `forum` | `batch_size`、`poll_interval_seconds`、`jitter_seconds`、`item_timeout_seconds` |
| `worker.delivery` | `batch_limit`、`poll_interval_seconds`、`jitter_seconds`、`request_timeout_seconds`、`finalize_timeout_seconds` |

热参数与启动租约冲突时（如论坛单项超时不小于租约）记录告警并继续使用启动值。

## 权限要求

| 对象 | 要求 |
| --- | --- |
| 配置目录、角色文件、`common.toml` | 不能被 group/other 写入 |
| `secrets.toml` | 只有 owner 可访问 |
| runtime 文件 | 恰好 0640；父目录不能 group/other 写入、不能是符号链接 |
| 维护凭据 | 规范化绝对路径、非符号链接、单硬链接、归当前账号且仅 owner 可读 |

`config check` 非严格模式把普通/敏感文件权限问题列为警告，`--strict` 视为错误；runtime 与维护凭据权限由 loader 强制检查，不受开关放宽，生产必须通过严格检查。`--strict` 另要求每角色 `[database].username = auction_runtime` 与不同的 `[database_roles] owner`。所有模式都经过同一套加载校验（语法、未知字段、区块归属、范围、三角色一致性、账号分工），启动与 CLI 给出同一组可行动错误，不回显 TOML 原文或秘密。

## 示例与首次设置

`ops/config/examples/` 同时是首次设置向导的生成模板（编译期内嵌）：每个赋值字段上方必须紧邻“可选/必填”中文注释（`check.sh config` 检查）；修改示例后向导生成的配置必须仍能通过 loader 校验；示例值仅用于演示。首次设置的目标目录必须尚不存在，其父目录须预先建立、为真实目录、归运行账号或 root 且禁止 group/other 写入；不能直接放在 `/tmp` 下。

## 排空与停止预算

三个角色各自在角色 TOML 中拥有 `[shutdown]`（不属于 common、不被 runtime 覆盖、无环境变量覆盖）。`drain_timeout_seconds` 默认 10（1–300）；`critical_write_timeout_seconds` Public/Admin 只接受 0，Worker 默认 60（0–300，0 完整选择旧排空路径）。切换需重启并遵守维护窗口。

Worker 新路径在首次停止时关闭入口，立即取消未登记交接的准备期工作并保留 claim 由租约恢复。delivery Send（含装备出库/解锁、附件与 COD）、Take/Return、discard 与 PM 的围栏调用开始即受保护：调用前登记，已关闭则不发起围栏；登记成功后的交接与关键段计入活动、阻止排空结束，仍须确认围栏提交才允许外写。论坛在持久 claim 下的首个实际 POST 前登记。全部已登记活动共享一次关键段截止，结果按原端口提交后才 complete，其余 drop 只记录未收口。之后执行普通资源收口。第二次信号立即强制收口、非零退出并跳过 tracing 刷新；预算耗尽同样非零退出。

外层停止上限须大于“普通 + 关键段 + tracing 上限（30 秒）+ 余量”：Public/Admin unit 45 秒、Worker 120 秒；`auction-server` 派生 `普通 + 关键段 + 30 + 5` 并求和（默认 195 秒，unit 225 秒）；CLI 读取 `TimeoutStopUSec` 再加 10 秒等待；stack unit 330 秒。提高应用预算须同步提高对应 unit、server 与 stack 上限；新程序不会修改已安装的主机 unit。

## 数据库运行时开关

这是部署字段来自 TOML 的明确行为覆盖例外，不引入环境变量，也不扩展 AdminRuntimeDocument：

| 名称 | 值 | 消费角色 / TOML 基线 |
| --- | --- | --- |
| readiness_audit_failure_mode | legacy/shadow/classified | 三角色 / readiness.audit_failure_mode |
| readiness_cache_enabled | bool | Public/Admin / readiness.cache_enabled |
| metrics_sli_snapshot_enabled | bool | Worker / metrics.sli_snapshot_enabled |
| mooglemail_poll_isolation_enabled | bool | Worker / worker.mooglemail.poll_isolation_enabled |
| metrics_reject_forwarded_requests | bool | 三角色 / metrics.reject_forwarded_requests（只允许在线收紧，`set false` 被命令层拒绝） |
| external_access_paused | bool | 三角色 / 无 TOML 基线（等价 false；clear/到期即恢复） |
| disposition_read_mode | legacy/shadow/classified | Admin/Worker / 无 TOML 基线（本版 shadow） |

优先级为最新且未过期的数据库 set → 启动 TOML 基线；clear、expire、到期或未设置均回落基线，修改 TOML 不清除无期限覆盖。三角色五秒刷新、单次一秒预算；首次失败保留基线，后续失败保留最后成功快照但仍按期限回落。切换不清零审计累计；策略版本变化立即使旧 HTTP 就绪缓存失效（Worker control 不使用该缓存）。`disposition_read_mode` 的 `shadow` 继续显示旧口径裁决值，只把逐项差异写进影子日志与指标（owner `disposition_reads`），经观察并由用户批准后才切 `classified`；两条口径必须由同一语句级快照配对读取。`external_access_paused` 的实际拦截由 Worker 执行（含经 control 代执行的 Admin/CLI 读取），见[ops/README.md#主动暂停外部访问w19](../README.md#主动暂停外部访问w19)。操作与 stale 诊断见[ops/README.md#运行时开关](../README.md#运行时开关)。

## 数据库连接分档

公共文件 `[database_profiles] mode` 为 `profiled`（默认）或 `legacy`；未知模式解析拒绝，连接参数只在建连时应用、修改须重启（受停机门禁，不接入在线开关）。`profiled` 按用途枚举设置 `application_name`（`auction-public/admin/worker`，CLI 见[apps/auction-cli/README.md#数据库连接用途](../../apps/auction-cli/README.md#数据库连接用途)）；关闭服务超时时只设名称并沿用 30 秒 acquire。`legacy` 固定池上限 16、acquire 30 秒，不设名称或会话参数。两种模式都保留池指标与事务观测，指标关闭不改变业务结果。

每个角色文件有 `[database_pool]`：`max_connections` 默认 16（物理池总上限，含长期 owner；Public 预留 1、Admin 0、Worker 按最坏情况 3——邮件、论坛与 LISTEN，扣除后至少剩 1 个普通连接）；`timeouts_enabled` 默认 false（与 legacy 同开会失败）；`acquire/statement/lock_timeout_millis` 无隐式默认，启用时须全部显式填写（候选 2000/5000/1000 不是默认开启值；范围 1–120000ms，lock 须短于 statement）；Public/Admin 的 acquire 必须严格短于有效 HTTP `request_timeout`（不保证多语句总耗时小于该期限）；`idle_in_transaction_session_timeout_millis` 默认 0（0–3600000），非零可能产生 `25P03`，按决策仍归 unknown 且可能粘滞阻断 readiness，须用户另行决定。

服务超时仅在以下条件全部满足后由用户启用：审计分类就绪已部署；观察完整 Final 与结算周期且差异仅在预期瞬时类别；三角色有效 readiness 为 `classified`；指标可采集。回退顺序为先关闭服务超时并重启，再考虑切回旧模式。

### 数据库账号分工

迁移 owner 只供 CLI 的建库、迁移、重建与恢复使用；Public/Admin/Worker 共用一个无 DDL 的运行账号。迁移 081 定义：`auction_runtime` 只有 `public` schema 的 `USAGE`，无 `CREATE`/`TRUNCATE`；普通表可读写，只追加事实表只可 `INSERT`，冻结证据与迁移表只可 `SELECT`，序列只给 `USAGE`/`SELECT`。未来 owner 新建表默认只给 `SELECT`，新增写入面须在迁移中显式 `GRANT`。完整清单见[docs/contracts/database.md#数据库账号与权限](../../docs/contracts/database.md#数据库账号与权限)。

维护策略在 `[database_profiles.maintenance]`：`max_connections` 默认 4、`acquire_timeout_millis` 默认 30000；profiled 维护档 `statement_timeout` 显式为 0，不套用服务档短超时；建库/重建 30 秒语句与 5 秒锁等待、恢复预检/盘点 5 秒语句与 1 秒锁等待保留。连接初始化用固定 GUC 名的单次 `set_config`，失败不交给 owner。

部署预算在 `[database_profiles.budget]`（规划声明）：`max_connections = 100`、`superuser_reserved_connections = 3`、`reserved_connections = 0`（reserve 待用户核实线上）、`max_concurrent_cli = 2`（只声明并行预算）、`external_reserved_connections = 8`（独立 guard、外部备份、exporter 与人工会话）。`config check` 汇总三角色池上限加 CLI 池与外部预留，与扣除 reserve 后的普通连接上限比较；profiled 超预算失败，legacy 只展示。每增加实例都要另计服务池。
