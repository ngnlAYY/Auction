# auction-config

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

公共、角色、敏感和 runtime TOML 的解析、校验及安全文件适配。
`src/setup/managed.rs` 拥有可选合并部署的运行目录推导；`FirstRunConfig::write_managed_to` 保留
原子发布与三角色校验，只将 Worker/Admin control socket 指向配置目录同级的私有目录。
配置 schema 不变，不把配置合并为含敏感值的单文件。

## 本页导航

- [源码入口](#源码入口)
- [职责与实现边界](#职责与实现边界)
- [解析与装配入口](#解析与装配入口)
- [公共、角色、敏感与 runtime 文件结构](#公共角色敏感与-runtime-文件结构)
- [敏感字段与数据密钥](#敏感字段与数据密钥)
- [PostgreSQL 维护敏感配置](#postgresql-维护敏感配置)
- [MoogleMail 本地联调凭据](#mooglemail-本地联调凭据)
- [Worker 控制与参数校验](#worker-控制与参数校验)
- [Runtime 文件锁](#runtime-文件锁)
- [告警与 HTTP 安全](#告警与-http-安全)
- [邮件轮询与预算](#邮件轮询与预算)
- [Runtime 生效范围](#runtime-生效范围)
- [环境变量边界](#环境变量边界)
- [凭据保护](#凭据保护)
- [兼容性](#兼容性)
- [维护与验证](#维护与验证)

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/config](src/config) | 主配置、敏感文件、默认值与约束 |
| [src/runtime](src/runtime) | Admin runtime 编解码、文件锁和原子替换 |
| [src/scheduling](src/scheduling) | Worker cron 与时区解析 |
| [src/setup](src/setup) | 基于通用模板的首次部署配置物化、敏感文件与三角色校验 |

## 职责与实现边界

TOML 配置解析与敏感配置加载。

## 解析与装配入口

源码布局：[src/lib.rs](src/lib.rs) 只保留模块声明和公共导出；主/敏感配置解析集中在 [src/config/](src/config/)，
其中 [src/config/settings/](src/config/settings/) 按拍卖策略、Worker/MoogleMail、HTTP 安全、敏感文件、应用
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
文件锁与原子替换集中在 [src/runtime/](src/runtime/)；其中 `validation.rs` 复用 Worker 的 cron 解析器，供候选校验、
文件读取/编码和启动 loader 共用。候选校验不执行 I/O，保存邮件参数不会触发远程轮询。

`AuctionConfig::policy()` 返回 `Result`：秒数超出 chrono 可表示范围时返回 `ConfigError`，
配置加载与预检不会因极端整数触发 panic；可表示的秒数继续由原有策略规则校验，不做截断。
文件加载先将角色文件定位为绝对路径，再以其所在目录解析公共、敏感与 runtime 引用；默认发现和
相对 `AUCTION_CONFIG_FILE` 因而使用相同基址。该步骤不解析 symlink，runtime 的路径和文件类型检查仍保留。

首次部署的 `FirstRunConfig::write_to` 只在目标明确不存在时生成配置；元数据读取失败不能当作
目标缺失，现有条目（包括悬空符号链接）拒绝覆盖。临时目录与 `runtime/` 子目录在创建时就分别
以 `0700`/`0750` 限制权限，目录权限仍受进程 umask 收紧；文件在私有临时目录内显式设为
非敏感 `0640`、敏感 `0600` 后同步，避免 `umask 077` 生成无法被正式 Admin 接受的 runtime 文件。
三角色 loader 和正式 runtime 文件读取校验均通过后，
依次同步 `runtime/`、临时根目录，再执行 Linux `RENAME_NOREPLACE` 发布并同步目标父目录。
发布前失败只清理本次临时目录；发布后父目录同步失败返回 `OutcomeUnknown`，保留目标供人工核对，
不得删除或回滚。该流程仍要求父目录受信且不被并发替换，不将路径探测当作对恶意目录竞态的隔离。
首次表单的 `public_listener_port`/`admin_listener_port` 必须非零且不同，只生成
`127.0.0.1:<port>`，Admin 角色与 runtime listener 同步写入，避免 runtime 覆盖用户选择。
它们不修改设置服务地址、浏览器 Origin 或代理信任策略，校验不探测端口占用与绑定权限。
首次排期候选使用 `AuctionScheduleConfig::validate` 复用正式计划规则，不依赖数据库：IANA 时区、
英文星期、同一周内至少提前两小时预览且开拍早于结束。首次表单额外要求精确 `HH:MM` 与时区
128 字节上限，关闭时也校验；不改变普通 loader 仅在排期启用时校验的既有语义。
计划写入公共 `[auction.schedule]`，生成的 runtime 明确保持 `operations.schedule.managed=false`，
防止模板覆盖；只影响后续周拍卖，不改已有策略快照、游标或租约。

## 公共、角色、敏感与 runtime 文件结构

三个角色文件都必须通过 `common_file` 引用同一份公共非敏感 TOML，并通过 `secrets_file`
引用各自的敏感 TOML；Admin/Worker 还可引用同一份由 Admin 管理的 runtime TOML。

完整配置模板分别维护在 [common.toml](../../ops/config/examples/common.toml)、
[public.toml](../../ops/config/examples/public.toml)、[admin.toml](../../ops/config/examples/admin.toml)、
[worker.toml](../../ops/config/examples/worker.toml) 和 [runtime/admin.toml](../../ops/config/examples/runtime/admin.toml)。
各文件按 owner 分别加载，不能拼成单文件。字段默认值、单位和启用条件随上述模板逐项维护；
角色关系和部署方法见[配置手册](../../ops/config/README.md)。


[Public 示例](../../ops/config/examples/public.toml)中的 `[session]` 数值是 Public 主配置默认值；Admin runtime 省略对应字段时使用独立旧版默认：
`idle_ttl_seconds = 900`、`absolute_ttl_seconds = 28800`。Public 仍保持 1800/604800。

## 敏感字段与数据密钥

敏感文件的稳定字段集合：`database`、兼容的 `database_url`、`redis_url`、`session_signing_key`、
`ehentai_cookie`、`mooglemail_cookie`、`admin_actor`、`admin_secret`、
`equipment_data_key_id`、`equipment_data_key`、`credential_data_key_id`、
`credential_data_key`、`user_login_data_key_id`、`user_login_data_key`、
`public_otlp_headers`、`admin_otlp_headers`、`worker_otlp_headers`。完整字段由
[敏感模型](src/config/settings/secrets/model.rs)定义。新增外部凭据必须显式
扩展该结构，禁止任意键值透传。Admin 凭据只由 Admin 装配根读取，`admin_secret` 至少需要
16 个字节；Admin 凭据和装备数据密钥的两项字段都必须成对出现，才能建立对应能力。

[secrets/database.rs](src/config/settings/secrets/database.rs) 拥有 PostgreSQL 的 `host`、`port`、
`username`、`password`、`name` 五字段校验和唯一 URL 转换。`PostgresConnectionConfig` 的
`Debug` 固定脱敏，未知/缺失字段和零端口拒绝，账号/密码按原文做一次百分号编码；库名作为单个
路径段编码，不允许点段触发默认库。新旧两种输入互斥，内部 `AppConfig::database_url()` 保持
原接口，部署目标比对、SQLx 与维护命令都消费同一结果。首次设置页也复用该转换并保存五字段
结构，兼容 URL 不被重写；需要 Unix socket 或显式 TLS profile 时继续使用兼容写法。

`equipment_data_key` 必须是 32 字节标准 Base64 数据密钥，`equipment_data_key_id` 是非秘密
轮换标识。Admin/Worker 用它封装或读取装备 key；Public 为 P09/P11 的受限 `no-store` 装备链接读取
使用匹配的解密密钥。密钥材料不得进入日志、DTO、审计 metadata 或浏览器，三进程部署预检核对其一致性。

`credential_data_key` 是 A32 MoogleMail/E-Hentai 凭据专用的 32 字节标准 Base64 数据密钥，
必须与 `credential_data_key_id` 成对提供，不能复用装备或用户登录 key。Admin 只有在
PostgreSQL 完整档且配置这对字段时才启用凭据导入/修改/删除；Worker 配置这对字段时会按
world/shared 读取唯一 active Cookie 并在内存中解密；`mail_cookie` 与 `forum_cookie` 各自全局
最多一条 active 记录。没有配置时 Worker 继续使用 `.keyenv` 或
`mooglemail_cookie` 的兼容入口。

相对 `secrets_file` 路径以主配置文件所在目录解析。敏感文件不得提交，部署时应限制为仅运行
账号可读。

## PostgreSQL 维护敏感配置

`load_postgres_maintenance_secrets(path)` 服务于 `auctionctl postgres-rebuild` 与 `postgres-bootstrap`。它解析一个与
常驻三进程敏感文件隔离的 TOML，接受 `[database]` 五项或兼容的 `database_url`，必须指向
`postgres` 维护库且不能混用；结构模板
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

## Worker 控制与参数校验

Worker 的 control socket（若启用）、Auction `concurrency`/`poll_interval_seconds`/`batch_limit`/
`jitter_seconds`/`item_timeout_seconds`、`recovery_retry_delay_seconds`、
`[worker.delivery]` 的批次/并发/poll 抖动/lease/续租/宽限/超时、`[worker.fulfillment]` 的
批次/并发/poll 抖动/lease/单项超时/重试，以及 `[worker.recovery]` 的同类调度边界、低余额阈值和
health 时间边界必须满足各自的路径/正数边界；Delivery 的续租间隔必须小于 lease，sending 宽限
必须大于完整请求超时；启用 MoogleMail 时 Delivery 完整请求超时还必须大于 MoogleMail 读取超时的两倍；Delivery 配置并发是上限，runner 还会取 transport 上限的较小值，当前
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

## Runtime 文件锁

Admin runtime 文件存储的更新/恢复锁使用非阻塞 `File::try_lock()`，从首次恢复检查持续持有到
文件和账本收口；竞争流程返回既有不可用错误，不阻塞等待。guard 绑定锁路径，取消或异常时
通过 RAII 释放；锁文件不会随每次操作删除，避免多进程锁住不同 inode。普通文件读取/替换仍
使用同一锁域，流程内部只调用已持锁接口，不重复获取锁。

## 告警与 HTTP 安全

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

## 邮件轮询与预算

启用 `worker.mooglemail` 后，Worker 会先为每个已配置 world 在 `mail_poll_state` 建立或读取
持久化状态，再通过独立租约读取 MoogleMail。解析成功的入站安全事实、`mail_commands` 幂等记录和 cursor 在同一
PostgreSQL 事务中提交；远端失败只更新退避和安全失败码。当前 Rust 首片已接入拍卖入站以及
用户注册/重绑业务处理；注册 key 由独立的数据密钥加密，数据库和 delivery outbox 不保存明文。
`[worker.mooglemail.diagnostics]` 用于保留 MoogleMail 拒绝或异常响应的原始数据，目录相对主 Worker
配置文件解析，默认保留 7 天且总量上限为 256 MiB；`retention_days` 只能为 1–365 天，
`maximum_total_bytes` 只能为 16 MiB–1 GiB。该归档仅属于 PostgreSQL 完整档，配置不新增 secret；
Worker 装配使用已有 credential data key 或 equipment data key 派生独立诊断加密子 key，Admin runtime
不会接管这组部署配置。
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
等级上下限各为 `0–1000`，`0` 分别表示不限制该边界；非零上限不能小于下限。
例如 `level_min = 300`、`level_max = 0` 只限制最低等级；启用任一等级边界时，缺少等级的装备仍拒绝。

## Runtime 生效范围

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

启用 Origin allowlist 而省略列表时，确定的 TCP listener 会推导规范 HTTP(S) Origin：
HTTP 的 80 端口与 HTTPS 的 443 端口省略，IPv6 和其他端口保留 URL 标准表示。
显式填写的来源仍须通过原有严格格式校验；Unix 或未指定 IP 的 listener 仍须显式填写。

## 维护与验证

修改本模块时同步对应[契约](../../docs/contracts/README.md)与源码入口；跨模块理由按[决策记录规则](../../docs/documentation.md)维护。

```bash
cargo check --locked -p auction-config
```

该命令只做静态编译；完整门禁见[开发指南](../../docs/development.md)，运行证据边界见[验证状态](../../docs/status.md)。
