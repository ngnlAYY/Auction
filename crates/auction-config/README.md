# auction-config

TOML 配置解析与敏感配置加载。

源码布局：`src/lib.rs` 只保留模块声明和公共导出；主/敏感配置解析集中在 `src/config/`，
其中 `src/config/settings/` 按拍卖策略、Worker/MoogleMail、HTTP 安全、敏感文件、应用
模型和校验职责拆分；通用设置再由 `settings/auction/{role,http,cache,policy,observability}.rs`
分别持有进程角色、监听、缓存、拍卖策略和可观测性配置；敏感配置继续按 `settings/secrets/{model,maintenance,keyenv,runtime}.rs`
划分模型、维护库密钥、`.keyenv` 和 Admin runtime owner；`settings/app/` 再按输入模型、加载、公共 getter 和最终构建拆分；Worker 配置进一步按 Auction、control、Delivery、Fulfillment、Recovery、
Forum、MoogleMail 和 Operations Alert 放在 `settings/worker/` 子目录，`settings/worker/runtime.rs`
只负责把 Admin 管理的旧版业务区块投影为 Worker MoogleMail 启动快照；`settings/operations/`
负责把显式接管的经济/周排期参数投影给 Admin/Worker，把论坛/告警/各任务循环/邮件预算投影给 Worker。
普通配置校验再按
拍卖、listener、安全、tracing 和路径 owner 放入 `settings/validation/{mod,auction,listener,security,tracing,paths}.rs`，
Worker 专属校验放入 `worker_validation/` 子目录；标准 cron 的公共模型、字段解析和 duration 解析分别位于
`src/scheduling/standard_cron/{mod,fields,duration}.rs`，Admin runtime TOML
文件锁与原子替换集中在 `src/runtime/`；其中 `validation.rs` 复用 Worker 的 cron 解析器，供候选校验、
文件读取/编码和启动 loader 共用。候选校验不执行 I/O，保存邮件参数不会触发远程轮询。

## 公共、角色、敏感与 runtime 文件结构

三个角色文件都必须通过 `common_file` 引用同一份公共非敏感 TOML，并通过 `secrets_file`
引用各自的敏感 TOML；Admin/Worker 还可引用同一份由 Admin 管理的 runtime TOML。以下代码块
是按 owner 汇总的字段速查，不是一份可直接使用的单文件配置；可运行的四文件组合见
`ops/config/examples/{common,public,admin,worker}.toml`：

```toml
# 必填：三个角色共同引用的公共非敏感 TOML 路径。
common_file = "common.toml"
# 必填：敏感 TOML 路径。
secrets_file = "secrets.toml"
# 可选：Admin/Worker 可指向同一份 Admin 管理的 runtime；示例位于 examples/runtime/admin.toml。
runtime_config_file = "runtime/admin.toml"
# common.toml 可选：tracing 过滤器。
log_filter = "info,auction=debug"

# 可选区块：Public/Admin HTTP listener；省略时使用各自 loopback TCP 默认值。
[listener]
# 可选：TCP host:port；Public 默认 127.0.0.1:8090，Admin 默认 127.0.0.1:8091。
address = "127.0.0.1:8090"
# 可选：Unix socket 所属 group；TCP 模式必须为空。
group = ""
# 可选：tcp 或 Linux 上的 unix；省略时为 tcp。
mode = "tcp"
# 可选：Unix runtime 目录；TCP 模式必须为空。
runtime_directory = ""
# 可选：直接位于 runtime 目录下的 Unix socket 路径；TCP 模式必须为空。
socket_path = ""

# common.toml 可选区块：进程本地 Prometheus 指标；Public 使用 HTTP，Worker 需同时启用 control 才有 UDS `/metrics`。
[metrics]
# 可选：是否启用固定 scrape endpoint；省略时关闭；启用时 Public/Admin listener 必须为 loopback/Unix。
enabled = false

# 可选区块：Public/Worker 的 OTLP/HTTP tracing exporter；省略时关闭。
[tracing]
# 可选：是否启用异步、有界队列的 exporter；省略时关闭。
enabled = false
# 可选：OTLP/HTTP endpoint；启用时必填，必须是 http/https URL。
endpoint = ""
# 可选：批处理等待、导出超时和停止刷新等待时间，单位为秒，范围 1-30。
batch_timeout_seconds = 2
export_timeout_seconds = 5
shutdown_timeout_seconds = 5
# 可选：单批最大 span 数和内存队列容量，范围分别为 1-512、1-4096。
max_export_batch_size = 64
queue_capacity = 256
# 可选：采样比例，百万分比；启用时范围 1-1000000，省略或关闭时可为 0。
sample_ratio_ppm = 0

# common.toml 可选区块：省略时关闭缓存。
[cache]
# 可选：disabled 或 redis，默认 disabled。
backend = "disabled"
# 可选：Public 规则读模型 TTL 秒数，范围 1-3600，默认 30。
public_rules_ttl_seconds = 30

# common.toml 可选区块：每周自动建立拍卖的时间表；默认关闭。
[auction.schedule]
# 可选：默认关闭，启用后 Worker 按下列本地周时间生成轮次。
enabled = false
# 可选：默认 UTC；可改为其他有效 IANA 时区。
timezone = "UTC"
# 可选：默认周四 22:00 开放预览，提供开拍前至少两小时的预览与持续录入窗口。
preview_day = "Thursday"
preview_time = "22:00"
# 可选：默认周五 UTC 00:00 正式开拍。
open_day = "Friday"
open_time = "00:00"
# 可选：默认周六 UTC 12:00 计划结束。
nominal_end_day = "Saturday"
nominal_end_time = "12:00"
# 可选：默认同时为 main 和 isekai 生成轮次。
worlds = ["main", "isekai"]

# 可选区块：创建拍卖时冻结的策略模板；省略时使用内置默认值。
[auction.economics]
# 可选：默认起始价，必须为正且按 1000 信用网格对齐，默认 1000。
start_price = 1000
# 可选：经济策略百万分比增量，范围 0-1000000，默认 50000。
bid_increment_ppm = 50000
# 可选：软截止窗口/延长秒数，默认均为 60。
soft_close_window_seconds = 60
soft_close_extension_seconds = 60
# 可选：取消截止窗口秒数，默认 86400。
cancel_cutoff_seconds = 86400
# 可选：代理开关、默认模式和 slow 间隔。
proxy_enabled = true
proxy_default_mode = "fast"
proxy_slow_interval_seconds = 600
# 可选：代理 outbid 使用次数上限，默认 3；0 表示不限制。
proxy_max_outbid_uses = 3
# 可选：final phase 领先秒数，范围 1-604800，默认 900（15 分钟）。
final_phase_lead_seconds = 900

[auction.settlement]
# 可选：版本与模式；模式只能是 hold_curve_v1 或 legacy_zero_v0。
version = 1
mode = "hold_curve_v1"
# 可选：hold_curve_v1 必须启用，legacy_zero_v0 必须关闭；默认启用。
compensation_pool_enabled = true
isekai_carrier_credit = 1
# 可选：PPM 锚点数组；省略时使用内置 hold curve。
anchors = [
  { hold_ppm = 0, adjustment_ppm = 80000 },
  { hold_ppm = 50000, adjustment_ppm = 64000 },
  { hold_ppm = 100000, adjustment_ppm = 48000 },
  { hold_ppm = 150000, adjustment_ppm = 32000 },
  { hold_ppm = 200000, adjustment_ppm = 16000 },
  { hold_ppm = 250000, adjustment_ppm = 0 },
  { hold_ppm = 300000, adjustment_ppm = -4000 },
  { hold_ppm = 400000, adjustment_ppm = -12000 },
  { hold_ppm = 500000, adjustment_ppm = -20000 },
  { hold_ppm = 1000000, adjustment_ppm = -20000 },
]

[auction.settlement.recovery]
# 可选：当前只支持七天、一次提醒的 remind_v1。
version = 1
mode = "remind_v1"
payment_timeout_seconds = 604800
reminder_limit = 1

# 可选区块：Auction 主循环；省略时 PostgreSQL 并发为 8、SQLite Lite 为 1，其余沿用旧版默认值。
[worker]
# 可选：主循环并发数；SQLite Lite 只允许 1。
concurrency = 8
# 可选：轮询、批次、随机抖动上限和单项总超时，默认 60/100/5/30。
poll_interval_seconds = 60
batch_limit = 100
jitter_seconds = 5
item_timeout_seconds = 30
# 可选：Recovery 各阶段依赖暂不可用时的持久化重试间隔，范围 1-86400 秒。
recovery_retry_delay_seconds = 60

# 可选区块：Delivery outbox 的独立轮询、并发、lease 和超时边界。
[worker.delivery]
# 可选：单轮上限和配置并发上限，默认 100/8；transport 可进一步收紧有效并发。
batch_limit = 100
concurrency = 8
# 可选：独立轮询、随机抖动上限、claim lease 和 sending 续租间隔秒数，默认 60/5/60/10。
poll_interval_seconds = 60
jitter_seconds = 5
lease_duration_seconds = 60
renew_interval_seconds = 10
# 可选：过期 sending 宽限、完整外部请求和终态写回超时秒数，默认 120/30/5。
sending_grace_seconds = 120
request_timeout_seconds = 30
finalize_timeout_seconds = 5

# 可选区块：Settlement fulfillment 的独立调度边界；默认 100/8/60/5/60/30/60。
[worker.fulfillment]
batch_limit = 100
concurrency = 8
poll_interval_seconds = 60
jitter_seconds = 5
lease_duration_seconds = 60
item_timeout_seconds = 30
retry_delay_seconds = 60

# 可选区块：Recovery 五阶段的独立调度、批次与 health 刷新边界；省略时使用默认值。
[worker.recovery]
# 可选：每个 recovery phase 单轮最多领取的记录数，范围 1-1000。
batch_limit = 100
# 可选：recovery action wave 并发数，范围 1-batch_limit，默认 8。
concurrency = 8
# 可选：独立轮询、随机抖动上限、action claim lease 和单项总超时秒数，默认 60/5/60/30。
poll_interval_seconds = 60
jitter_seconds = 5
lease_duration_seconds = 60
item_timeout_seconds = 30
# 可选：MoogleMail Balance 低水位阈值，低于该值标记为 low，默认 100 credits。
low_balance_threshold = 100
# 可选：recovery health 远端余额刷新间隔，范围 1-86400 秒，默认 60。
health_refresh_interval_seconds = 60
# 可选：余额证据超过该年龄标记为 stale，范围不小于刷新间隔且不超过 86400 秒，默认 300。
evidence_stale_after_seconds = 300

# 可选区块：Worker control；默认关闭。启用后使用安全 Unix socket 和显式 UID/GID peer allowlist，并提供 live/ready/status、`/metrics` 和唤醒。
[worker.control]
# 可选：是否启用 Worker control；默认 false。
enabled = false
# 可选：socket 所属 Unix group；默认 auction-worker-control。
group = "auction-worker-control"
# 可选：内核认证的 peer GID/UID 白名单；启用时至少填写一项实际数字 ID。
allowed_peer_gids = []
allowed_peer_uids = []
# 可选：预先建立的安全 runtime 目录；默认 /run/hv-auction/worker。
runtime_directory = "/run/hv-auction/worker"
# 可选：规范化绝对 socket 路径；默认 /run/hv-auction/worker/control.sock。
socket_path = "/run/hv-auction/worker/control.sock"

# 可选：MoogleMail 入站轮询；默认关闭。
[worker.mooglemail]
# 可选：启用后只在 PostgreSQL 完整档执行远程 GET。
enabled = false
# 可选：持久化邮箱标识。
mailbox_key = "mooglemail:main"
# 可选：游戏世界，只能是 main 或 isekai。
world_id = "main"
# 可选：拍卖邮件起拍价覆盖值；省略时继承 [auction.economics] 的 start_price。
# start_price = 1000
# 可选：相对主配置解析的 `.keyenv` 路径；或使用敏感 mooglemail_cookie。
keyenv_file = "../../../.keyenv"
# 可选：固定生产 origin 或回环联调 origin，以及传输响应、限流和请求超时边界。
endpoint = "https://hentaiverse.org"
maximum_response_bytes = 8388608
minimum_request_interval_millis = 1000
request_timeout_seconds = 15
# 可选：单次读取上限和有界分页上限。
batch_size = 100
maximum_pages = 20
# 可选：每个 world 共用的标准五字段 cron；默认每 5 分钟，也支持旧版描述符、@every 和时区前缀。
schedule = "*/5 * * * *"
# 可选：cron 后随机抖动和可续期 claim 边界。
jitter_seconds = 5
lease_duration_seconds = 60
renew_interval_seconds = 10
# 可选：远程读取和单封邮件业务处理的内联尝试次数与单次总超时。
fetch_attempts = 3
fetch_timeout_seconds = 30
process_attempts = 3
item_timeout_seconds = 30
# 可选：单批远程拉取及业务处理总预算秒数，默认 600，至少覆盖一次 fetch 与一次 item 超时。
batch_timeout_seconds = 600
# 可选：内联与持久化指数退避范围。
retry_base_seconds = 1
retry_maximum_seconds = 30
# 可选：allowlist-first 详情读取；生产环境不要关闭 allowlist。
sender_allowlist = []
sender_allowlist_disabled = false

# 可选区块：拍卖装备过滤；默认关闭，但来源仍必须标记为可交易。
[worker.mooglemail.equipment_filter]
enabled = false
qualities = []
prefixes = []
suffixes = []
categories = []
equipment_types = []
level_min = 0
level_max = 0

# 可选区块：MoogleMail 业务通知模板；省略字段使用旧版默认英文文本。
[worker.mooglemail.mail_templates]
auction_assets_rejected = "Auction assets rejected"
auction_no_attachments = "Auction has no attachments"
auction_equipment_rejected = "Auction equipment rejected: {reason_code}"
user_assets_rejected = "User assets rejected"
register_key = "Registration key {login_key} for {username}"
register_result = "Registration result {result_code} {username}"
rebind_result = "Rebind result {result_code} {username}"
settlement_winner_equipment_cod = "Auction item {item_name}; COD {amount}; reference {reference}"
settlement_payment_carrier = "Auction payment request {amount}; reference {reference}"
settlement_payment_reminder = "Auction payment is overdue; reference {reference}"
settlement_equipment_handoff = "Auction item {item_name}; reference {reference}"
settlement_seller_payout = "Auction seller payout {amount}; reference {reference}"
settlement_buyer_refund = "Auction buyer refund {amount}; reference {reference}"
operations_alert = "Operations alert: {summary}"

# 上述 world/sender、equipment_filter 与 mail_templates 是 Worker 主配置的兼容回退。
# Admin 管理的共享 runtime 同时包含三个业务区块时会覆盖它们；全局 enabled、transport、
# 凭据、调度和 mailbox durable identity 始终由本主配置拥有。

# 可选：额外启用一个世界；每个条目拥有独立 mailbox_key、cursor 和 allowlist。
# 顶层 world_id 代表第一个世界，不能重复配置 world_id 或 mailbox_key。
# [[worker.mooglemail.additional_worlds]]
# world_id = "isekai"
# mailbox_key = "mooglemail:isekai"
# sender_allowlist = []
# sender_allowlist_disabled = false

# 可选区块：运维告警；默认关闭，只在 PostgreSQL 完整档运行。
[worker.operations_alert]
# 可选：启用后必须填写 recipient。
enabled = false
# 可选：scan、dispatch 和 reconcile 三类独立循环的间隔秒数，默认 60。
scan_interval_seconds = 60
# 可选：每轮完成后附加的随机抖动上限秒数，不能超过 scan_interval_seconds，默认 5；0 表示关闭抖动。
jitter_seconds = 5
# 可选：来源超时、批次窗口、冷却和 claim 租约秒数。
source_timeout_seconds = 5
batch_window_seconds = 120
cooldown_seconds = 1800
claim_lease_seconds = 120
retry_delay_seconds = 300
# 可选：扫描、批次和正文边界。
max_signals_per_scan = 64
max_batch_items = 20
max_body_bytes = 4096
# 可选：delivery/fulfillment/recovery 水位阈值。
delivery_pending_count_threshold = 1
delivery_pending_age_seconds = 900
delivery_manual_reconcile_count_threshold = 1
fulfillment_backlog_count_threshold = 1
fulfillment_backlog_age_seconds = 900
recovery_overdue_count_threshold = 1
recovery_overdue_age_seconds = 900
# 可选：通知世界和收件人。
world_id = "main"
recipient = ""

# 可选区块：HTTP 安全策略。
[http_security]
# 可选：是否启用 origin allowlist；默认 true。
allowed_origins_enabled = true
# 可选：允许的精确 HTTP(S) origin。
allowed_origins = ["http://127.0.0.1:8090"]
# 可选：登录限流次数。
login_rate_limit = 10
# 可选：变更限流次数；Public realtime 连接尝试使用相同上限但独立账本。
mutation_rate_limit = 60
# 可选：限流窗口秒数。
rate_limit_window_seconds = 60
# 可选：Cookie Secure 属性；生产环境应开启。
cookie_secure = false
# 可选：普通请求处理超时秒数；默认 15，范围 1-120，WebSocket 路径豁免。
request_timeout_seconds = 15
# 可选：可信 TCP 代理的规范 CIDR；默认空，最多 64 项。
trusted_proxy_cidrs = []
# 可选：信任 Unix socket 上游；只允许有效的 Public/Admin Unix listener 启用。
trusted_unix_proxy = false

# 可选区块：会话策略。
[session]
# 可选：空闲过期秒数。
idle_ttl_seconds = 1800
# 可选：绝对过期秒数。
absolute_ttl_seconds = 604800
# 可选：公开会话并发上限。
max_public_sessions = 5
```

上面的 `[session]` 数值是 Public 主配置默认值；Admin runtime 省略对应字段时使用独立旧版默认：
`idle_ttl_seconds = 900`、`absolute_ttl_seconds = 28800`。Public 仍保持 1800/604800。

敏感文件的稳定字段集合：`database_url`、`redis_url`、`session_signing_key`、
`ehentai_cookie`、`mooglemail_cookie`、`admin_actor`、`admin_secret`、
`equipment_data_key_id`、`equipment_data_key`、`credential_data_key_id`、
`credential_data_key`。新增外部凭据必须显式
扩展该结构，禁止任意键值透传。Admin 凭据只由 Admin 装配根读取，`admin_secret` 至少需要
16 个字节；Admin 凭据和装备数据密钥的两项字段都必须成对出现，才能建立对应能力。

`equipment_data_key` 必须是 32 字节标准 Base64 数据密钥，`equipment_data_key_id` 是非秘密
轮换标识。两者只由 Admin 装配根读取，用于封装装备 key；任何一个字段都不得进入日志、DTO、
审计 metadata 或浏览器。

`credential_data_key` 是 A32 MoogleMail/E-Hentai 凭据专用的 32 字节标准 Base64 数据密钥，
必须与 `credential_data_key_id` 成对提供，不能复用装备或用户登录 key。Admin 只有在
PostgreSQL 完整档且配置这对字段时才启用凭据导入/修改/删除；Worker 配置这对字段时会按
world/shared 读取唯一 active Cookie 并在内存中解密；`mail_cookie` 与 `forum_cookie` 各自全局
最多一条 active 记录。没有配置时 Worker 继续使用 `.keyenv` 或
`mooglemail_cookie` 的兼容入口。

相对 `secrets_file` 路径以主配置文件所在目录解析。敏感文件不得提交，部署时应限制为仅运行
账号可读。

## PostgreSQL 维护敏感配置

`load_postgres_maintenance_secrets(path)` 只服务于 `auctionctl postgres-rebuild`。它解析一个与
常驻三进程敏感文件隔离的 TOML，唯一必填字段是指向 `postgres` 维护库的 `database_url`；结构模板
位于 `ops/config/examples/secrets/postgres-rebuild.toml`。路径必须是规范化绝对路径，目标必须是
普通非 symlink 文件，父目录不得允许 group/other 写入；Unix 文件必须归当前有效用户所有、只有
一个 hard link，并去除全部 group/other 权限位。类型的 `Debug` 固定脱敏，解析错误
只报告路径；调用方只能通过显式 getter 短暂取得 URL，不能写入日志、命令参数、receipt 或 DTO。

## MoogleMail 本地联调凭据

用户明确提供的本地 `.keyenv` 可作为 MoogleMail 联调的窄凭据通道。`auction-config` 的
`load_mooglemail_keyenv(path)` 只接受两个固定键：`hv-ck-key1` 是 Worker 运行账号，
`hv-ck-key2` 只供显式隔离验证；生产 Worker 只使用前者，不把两者组成候选，也不会自动切换；
未知键、重复键、空值、控制字符和不安全权限都会失败关闭。Unix 文件必须是 owner-only
（建议 `chmod 600 .keyenv`）；值可使用成对单引号或双引号包裹。文件本身已被 Git 忽略，不能提交。

该文件不是通用环境变量覆盖，也不能承载数据库、Redis、会话或其他业务凭据。读取后只能在
装配 MoogleMail 适配器时转换为脱敏 Cookie 值对象，禁止写入日志、快照、DTO、数据库或错误
消息；生产部署仍优先使用敏感 TOML 的显式 `mooglemail_cookie` getter。

Worker 的 control socket（若启用）、Auction `concurrency`/`poll_interval_seconds`/`batch_limit`/
`jitter_seconds`/`item_timeout_seconds`、`recovery_retry_delay_seconds`、
`[worker.delivery]` 的批次/并发/poll 抖动/lease/续租/宽限/超时、`[worker.fulfillment]` 的
批次/并发/poll 抖动/lease/单项超时/重试，以及 `[worker.recovery]` 的同类调度边界、低余额阈值和
health 时间边界必须满足各自的路径/正数边界；Delivery 的续租间隔必须小于 lease，sending 宽限
必须大于完整请求超时；Delivery 配置并发是上限，runner 还会取 transport 上限的较小值，当前
MoogleMail transport 因账号级 compose 可变状态固定为 1；Fulfillment 和 Recovery 的单项超时必须小于各自 lease；
其中 recovery 重试间隔不能超过 86400 秒；省略 Auction concurrency 时，配置层按 PostgreSQL=8、
SQLite Lite=1 计算有效值，显式 SQLite 值也只允许 1，以落实单主机、单写者约束。轮询间隔只负责唤醒持久化调度器，
不能替代数据库 due/租约状态。MoogleMail 每个 world 使用独立循环和手动唤醒器，读取与单封业务
处理分别按配置进行有界内联重试；批次执行期间周期续租，续租失败后取消陈旧 future，不能绕过
token、owner 和版本 fence 提交 cursor。
MoogleMail `batch_timeout_seconds` 范围为 1–86400 秒，默认 600 秒，且不得小于
`fetch_timeout_seconds + item_timeout_seconds`；它覆盖拉取和业务处理的重试总耗时，独立于
可续期 lease。预算耗尽后只提交已完成邮件前缀及其 cursor，未完成邮件不会被跳过。该字段也可
由 `operations.mail.managed` 接管；旧 runtime 文档省略字段时使用默认值，序列化继续省略默认值
以保持既有 fingerprint 和历史命令摘要。
`fetch_timeout_seconds` 仍限制每次拉取尝试；Worker 将其绝对截止时刻传给 gateway，详情预算
耗尽时返回已解析的最旧前缀，不要求默认 100 封详情全部在一次 30 秒尝试内完成。空收件箱或无
allowlist 命中是正常空结果，列表尚未完整定位 cursor 或末尾时仍不得推进。

Admin runtime 文件存储的更新/恢复锁使用非阻塞 `File::try_lock()`，从首次恢复检查持续持有到
文件和账本收口；竞争流程返回既有不可用错误，不阻塞等待。guard 绑定锁路径，取消或异常时
通过 RAII 释放；锁文件不会随每次操作删除，避免多进程锁住不同 inode。普通文件读取/替换仍
使用同一锁域，流程内部只调用已持锁接口，不重复获取锁。

启用 `[worker.operations_alert]` 后，PostgreSQL Worker 会读取 process、delivery、fulfillment 和
recovery 四类安全水位，按旧版 15 类 signal 和阈值生成持久化告警，使用 cooldown/lease/重试
批次通过 MoogleMail delivery outbox 发送，并在 delivery 终态后完成成功/失败/人工对账。通知正文
只包含冻结的安全摘要，不包含收件人以外的凭据、原始错误或数据库 payload；SQLite Lite 明确
返回能力缺失。

`http_security` 只保存 origin、速率限制、Cookie 属性、普通请求超时和可信代理 CIDR 等非敏感
策略；默认不信任任何 `X-Forwarded-For`。`trusted_unix_proxy` 仅允许有效的 Public/Admin Unix
listener 启用，TCP listener 必须通过规范且不重复的 `trusted_proxy_cidrs` 明确信任上游。
Admin 主配置 fallback 与 runtime 覆盖值分别校验；`AppConfig.listener` 保存最终生效的 TCP/Unix
配置，供监听器装配、origin/metrics 校验和部署预检共用。
`session` 保存 Public 与 Admin 会话的空闲/绝对时限及 Public 并发上限。登录 key、Admin secret、
会话 token 和数据库连接串不得写入主配置。

启用 `worker.mooglemail` 后，Worker 会先为每个已配置 world 在 `mail_poll_state` 建立或读取
持久化状态，再通过独立租约读取 MoogleMail。解析成功的入站安全事实、`mail_commands` 幂等记录和 cursor 在同一
PostgreSQL 事务中提交；远端失败只更新退避和安全失败码。当前 Rust 首片已接入拍卖入站以及
用户注册/重绑业务处理；注册 key 由独立的数据密钥加密，数据库和 delivery outbox 不保存明文。
结算退信以及 Sent/Balance 回执对账已由 PostgreSQL Worker owner 接入。启用 `[worker.forum]`
后，PostgreSQL Worker 会装配 E-Hentai 主题发布和回帖导入；唯一论坛 Cookie 可从独立密文记录读取，
或使用敏感配置的兼容 getter。`worker.forum.public_base_url` 可选，填写无路径的公开端
origin 后会把旧版 `/?auction=...&world=...` 相对链接解析为主题正文中的绝对链接；
`poll_interval_seconds` 和 `jitter_seconds` 默认分别为 300 与 5，独立于 Auction 生命周期循环；成功
扫描会持久化下一次 due。`maximum_topic_pages` 默认 10、允许范围 1-20，越界主题不会被截断处理。
`item_timeout_seconds` 同时控制单个主题发布和回帖导入总时限，必须覆盖分页读取、固定楼层写入、
共享请求间隔与收口预算，并小于独立 `lease_duration_seconds`。Forum 使用自己的顺序循环，发布失败
不会跳过回帖导入，也不会终止其它 Worker owner。论坛完整档不在 SQLite Lite 中伪装提供。
`sender_allowlist_disabled` 必须显式设为 true 才会读取非 allowlist 发件人的详情。
`equipment_filter` 启用后按品质、前后缀、分类、类型和等级返回稳定拒绝原因；被拒绝且邮件可退回
时会安排退回和通知，不能退回时会 fail closed，不会把不符合策略的装备写入拍卖。

Admin 与 Worker 主配置的 `runtime_config_file` 可以指向同一份、由 Admin 管理的非敏感 runtime
TOML，仓库模板位于 `ops/config/examples/runtime/admin.toml`。该文档由严格应用模型解析，包含
Admin listener、安全策略、会话时限、metrics/tracing 公共参数、Worker control socket、可选的
dashboard MoogleMail world/mailbox 清单，以及旧版 `worlds`、发件人 allowlist、
`equipment_filter`、14 个 `mail_templates` 和可选的六组 `operations`，
不包含 database URL、Cookie、密码、签名密钥或装备数据密钥。PostgreSQL 完整档通过 Admin
HTTP 的 GET/validate/PUT 读取和原子更新；更新写入后必须重启受影响的 Admin/Worker，SQLite Lite 不提供该
完整更新能力。运行中的 runtime 文件必须是受信 regular file，目录不可被 group/world 写入，
文件权限为 `0640`；仓库示例只用于复制模板。

当 Admin 配置了 `runtime_config_file` 时，runtime 文件中的 `[metrics]` 和 `[tracing]` 参数
覆盖公共文件或角色文件的对应字段，确保 runtime 页面修改后重启真正改变 Admin 的 `/metrics` 与
OTLP surface；Public/Worker 的 `[metrics]` 来自公共文件，`[tracing]` 继续来自各自角色文件。Worker 先消费共享 runtime 的
world/sender/filter/template 业务区块；三个区块完全缺失时保留 Worker 主配置，部分存在时失败关闭。
完整业务区块存在时，它们覆盖 Worker 直接配置，但不改变 MoogleMail transport、凭据、调度、mailbox identity
或全局 `enabled`。runtime `worlds[*].enabled` 只是 Worker/MoogleMail 入站和每周调度的候选开关，
不是数据库权威业务关闭；实际新场次 bootstrap 还必须与 `[auction.schedule].worlds` 及
`worlds.enabled=true` 取交集。数据库状态由 Admin A40/A41 独立管理，不能通过保存 TOML 绕过活动
拍卖 blocker 或 `policy_revision` fence。三类进程的 OTLP headers 只能放在敏感 TOML 的角色专属字段中，不能写入 runtime
或公共/角色配置。

`[operations]` 分成 `economics`、`schedule`、`forum`、`alerts`、`worker`、`mail` 六组，每组的
`managed` 默认关闭，沿用原公共/角色 TOML；开启后才覆盖该组已公开字段。Worker 组还包含
`delivery`、`fulfillment` 和 `recovery` 三套独立循环边界；Mail 组独立接管邮件 cron 与读取/处理预算。
旧文件完全省略 `operations` 或 `operations.mail` 时不改变
fingerprint 或历史命令摘要；HTTP 补齐的是未接管候选，不能据此显示进程的当前生效值。
新候选使用同一文档及 cron 校验；启动完成投影后还执行凭据、邮件 world、后端能力和 SQLite 主并发等
部署关联校验。`economics` 只影响新拍卖的冻结模板；`schedule` 只用于后续轮次生成。
这两组变更须重启 Admin 与 Worker，其余四组须重启 Worker。部署预检在接管任一组时要求
Admin/Worker 引用同一份 runtime 文件，并核对两者有效拍卖模板和排期一致。

## 环境变量边界

`AUCTION_CONFIG_FILE` **只用于选择主配置文件**，不能覆盖任何单个配置项。业务模块也不得自行
读取环境变量。

## 凭据保护

- 敏感值包装为 `SecretString`，其 `Debug` 输出为 `[REDACTED]`。
- `AppConfig` 的 `Debug` 只报告 `database_configured` 布尔值，不输出连接串。
- TOML 解析失败时只报告文件路径，**不回显原始行**——解析器错误可能带出连接串或 Cookie。
- 读取凭据必须通过显式 getter（`database_url()`、`session_signing_key()` 等）；调用方不得
  把返回值写入日志、错误消息或 DTO。

## 兼容性

当前格式不兼容旧的单文件敏感字段，旧格式必须以明确错误失败。配置字段变化必须同时更新解析
代码、`ops/config/examples/`、相关 README 和部署说明。
