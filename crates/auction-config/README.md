# auction-config

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [契约](docs/contracts/README.md)

公共、角色、敏感和 runtime TOML 的解析、校验及安全文件适配。本 crate 拥有部署格式与配置 I/O，向应用层提供
已校验值；业务模块不得自行读取文件或环境变量。完整字段、默认值、单位与启用条件统一维护在
[配置手册](ops/config/README.md)及其模板中。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/config](crates/auction-config/src/config) | 配置加载、模型、默认值与角色约束 |
| [src/runtime](crates/auction-config/src/runtime) | Admin runtime 编解码、文件锁、候选校验与原子替换 |
| [src/scheduling](crates/auction-config/src/scheduling) | Worker cron、时区与下一次触发时刻 |
| [src/setup](crates/auction-config/src/setup) | 首次配置物化、权限、原子发布与三角色校验 |

## 解析与装配入口

`config/settings/` 按 `auction`、`worker`、`secrets`、`app`、`validation` 和 `operations` 分工。角色文件先定位为
绝对路径，相对公共/敏感/runtime 引用均以其目录解析，该步骤不解除 symlink 安全检查。`AuctionConfig::policy()`
返回 `Result`，自身拒绝未知结算/恢复模式及不可表示的时间范围，不依赖外层先校验、不静默替换策略。
新拍卖结算默认 `version=2/mode=hold_curve_v2`，卖方全额收款；旧 v1 只保留冻结快照读取，不能作为新建模板。
显式旧配置须按[升级说明](ops/README.md#卖方全额结算升级)修改 version/mode，不静默替换，也不回写已有场次。
新场次 soft-close 窗口/延长默认均为 900 秒，
common、runtime 候选与首次设置内嵌模板同步；升级不覆盖文件中显式的 60，须核对 managed 有效来源并手动更新
（步骤见[配置手册](ops/config/README.md#缓存与拍卖模板)）。

角色、公共及普通敏感 TOML 各限 4 MiB；`config/reader.rs` 在同一描述符检查普通文件并有界读取，Unix 以
`O_NONBLOCK` 打开，避免 FIFO 在类型检查前阻塞。普通配置继续允许部署侧 symlink；默认发现只有明确 `NotFound`
才使用内置默认值，目录、断链和访问错误均失败关闭，不能当成“没有配置”。
公共 `log_filter` 在其余配置校验完成后使用与进程初始化相同的 `EnvFilter::try_new` 检查语法，
使配置预检提前拒绝原本会在 tracing 初始化失败的表达式；错误固定脱敏，不回显输入或底层解析原因。
省略时继续使用角色默认过滤器；显式值沿用该解析器的接受范围，不额外限制长度、空白或空串。
Admin 可选 `worker_config_file` 由配置 owner 有界读取非敏感 Worker 文件，仅冻结邮件启用状态及跨组预算；
不加载 Worker 的 common/secrets，引用须与 Admin runtime 同源。候选按真实启动快照及 managed 覆盖验证，
缺少可证明基值时明确拒绝，规则及升级步骤见[配置手册](ops/config/README.md#文件职责)。

各角色仍独立加载并校验其文件快照，不缓存跨次文件内容。Admin 在窄投影完成后接管已解析 runtime 文档；
Public/Worker 直接转移 bot 名单，Worker 在其他投影完成后消费该文档，转移发件人名单、装备过滤列表与邮件模板。
Worker 的 world 投影只借用部署邮箱标识，避免为查找邮箱复制发件人名单。
告警 world 成员检查同样只借用世界标识，不为成员判断克隆邮箱、发件人名单或排序。
Admin dashboard、Worker 周排期/恢复健康与 CLI 的 world/mailbox 查询复用 `configured_mailboxes` 借用迭代器，
与完整 world 配置共用唯一稳定分组，不分配临时列表；完整发件人名单仅在邮件业务确实需要持有时复制。
Worker `mail_templates` 直接使用应用层 `AdminRuntimeMailTemplatesConfig`：TOML 字段闭集、默认正文与占位符校验和
Admin runtime 同源，runtime 投影整体替换模板而不逐字段复制；结算模式与世界名称经领域稳定名称表解析，
Unix 监听属组复用应用层 `valid_listener_group`，配置层不再保留第二份规则。
没有 Admin runtime 覆盖时复用已完成的 HTTP 安全校验，不重复解析同一组 Origin/CIDR；有覆盖时仍先校验主配置
fallback，再独立校验最终投影，保留原错误优先级与 Unix proxy、metrics 的有效 listener 门禁。

`FirstRunConfig::write_to` 只向明确不存在的目标发布：私有临时目录中生成文件、固定非敏感 `0640`/敏感 `0600`，
复用三角色 loader 与正式 runtime 校验后同步目录，以 Linux `RENAME_NOREPLACE` 发布并同步父目录。发布前只清理
本次临时目录；发布后确认失败返回 `OutcomeUnknown` 并保留目标，不回滚，父目录必须受信。
`write_managed_to` 只额外推导配置目录同级的私有 control 目录，不合并敏感配置。
首次生成的论坛 `public_base_url` 与已校验 Public origin 一致，不继承示例主机。
内嵌模板按完整 TOML 文档反序列化，再修改字段；不能使用仅解析单值的 `Value::FromStr`。
首次设置在发布前拒绝正式 loader 必拒的控制组名前导下划线，不先接受不可发布的候选。
排期候选统一校验周计划与早鸟期，生成 `common.toml` 时整组序列化 `AuctionScheduleConfig`，不再逐字段复制而遗漏
`early_bird`；Web 首次设置仍只提供默认关闭的早鸟候选，不增加表单字段。

## 文件职责与敏感边界

三个角色必须通过 `common_file` 引用同一公共非敏感 TOML，各自通过 `secrets_file` 引用敏感 TOML；
三角色可共用 `runtime_config_file`，文件按 owner 分别加载，禁止拼成敏感单文件。模板入口：
[common](ops/config/examples/common.toml)、[Public](ops/config/examples/public.toml)、[Admin](ops/config/examples/admin.toml)、
[Worker](ops/config/examples/worker.toml)、[runtime](ops/config/examples/runtime/admin.toml)。Public 部署会话仅配置并发上限，默认 5；
登录时长由账号设置，默认永久。旧 Public `idle_ttl_seconds` / `absolute_ttl_seconds` 明确拒绝，不静默忽略。
Admin runtime 的独立默认值仍为 900/28800 秒；角色解析与迁移说明见[配置手册](ops/config/README.md#http-listener-与安全)。

[settings/secrets](crates/auction-config/src/config/settings/secrets) 严格定义敏感字段，不接受任意键值透传。
PostgreSQL `[database]` 接收 host、port、username、password、name 并统一编码为 URL，与兼容 `database_url` 互斥；
需要 Unix socket 或显式 TLS profile 时使用兼容 URL。旧 URI 必须明确指定 host/socket 和库名，省略端口固定为 5432；
显式端口必须非零，不通过 PG* 环境补齐目标。缩略 URI、fragment、控制字符和无效百分号/UTF-8 转义失败关闭，凭据编码
与其余驱动参数保持原文，输入与 `Debug` 均不得泄密。Admin actor/secret
必须成对配置，缺少任一字段在加载敏感模型时失败，不能静默关闭登录；secret 至少 16 字节。装备、外部凭据、用户登录
key 各自使用独立数据密钥与非秘密 key ID，密钥为 32 字节标准 Base64，不得互相复用。敏感文件禁止提交、限运行账号
访问，字段与跨角色一致性见[敏感配置](ops/config/README.md#敏感字段)。`AUCTION_CONFIG_FILE` 仅选择角色配置文件，
不覆盖任何单个字段。
无百分号转义的连接串校验与账号、密码转义直接借用输入，减少启动时无用的敏感文本副本。

## 维护凭据与本地联调

`load_postgres_maintenance_secrets` 为 bootstrap/rebuild 独立加载指向 `postgres` 维护库的 TOML：要求规范绝对路径、
普通非 symlink 文件和安全父目录，Unix 下须归当前有效用户、单 hard link、无 group/other 权限；维护 URL 仅经显式
getter 短暂使用，不进入命令参数、日志、receipt 或 DTO（见[维护凭据](ops/config/README.md#postgresql-维护凭据)）。
`.keyenv` 与维护 TOML 各限 64 KiB，均以非阻塞方式打开后核验普通文件，避免特殊文件卡住启动或运维。
`load_mooglemail_keyenv` 只接受 `.keyenv` 的 `hv-ck-key1` 与 `hv-ck-key2`，拒绝未知/重复键、空值、控制字符和不安全权限。
保留 `.keyenv` 指向私有普通文件的 symlink 用法，但权限检查与读取使用同一描述符，不在检查后重新打开路径。
Worker 只使用 key1，key2 仅供显式隔离验证，不能作为候选或自动回退。

## 参数与预算校验

配置层校验 control 路径、正数范围、lease/续租/单项 timeout 关系及后端能力。Auction 默认并发为 8；
Delivery 有效并发取配置与 transport 上限的较小值，MoogleMail transport 当前为 1；启用 MoogleMail 时发送总预算
必须大于两倍读取预算，sending grace 必须大于发送预算。每 world 独立 mailbox、轮询状态与 cron，
`batch_timeout_seconds` 默认 600、范围 1–86400，不得小于 fetch 与 item 预算之和；预算耗尽只能提交已完成连续前缀，
未完成邮件不能被 cursor 跳过。定时器只唤醒持久化任务，不能代替 due、租约或游标（值见
[Worker 预算](ops/config/README.md#worker-循环预算)）。交给 croner 4.0.1 前仍先做严格五字段语法检查，逐项读取固定五个
字段，不为多余输入分配字段列表；英文月份/星期名称按 ASCII 忽略大小写匹配。不开放上游新增昵称或宽松输入，
描述符、`@every` 与时区契约保持不变。`worker.forum.verify_post_body` 是默认关闭的部署侧正文校验开关，
不属于 runtime 的 `operations.forum` 接管范围。
cron 的五个规范字段直接追加到最终缓冲，不建立逐字段临时 String；原语法、时区及 DOM/DOW 语义保持。
邮件模板闭集校验拒绝任意位置的孤立右花括号，与 runtime 和 transport 边界一致。
发件人名单与装备 token 使用借用原文的集合判重，避免最大名单的平方扫描；
保持大小写、输入顺序、256/1000 项预算与错误边界，不复制名单字符串。

## Runtime 文件与生效范围

Admin runtime 更新/恢复使用非阻塞 `File::try_lock()`，从恢复检查持续持有至文件与账本收口；竞争返回不可用，
guard 取消后由 RAII 释放，锁文件不逐次删除。锁文件须由有效 UID 拥有、无 group/other 权限且为单链接普通文件，
打开时拒绝 symlink。Unix runtime 文件固定为 0640；三角色启动和管理读取共用同一描述符的文件类型、权限及
4 MiB 预算校验，拒绝 symlink/FIFO，实际最多读入上限加一字节。管理读写另要求文件由当前有效 UID 拥有；
只读角色装配不拿运维进程 UID 代替服务账号，root 部署命令仍按实际 unit `User` 校验目标服务 owner。
替换先写临时文件并固定权限，一次文件 fsync 后才 rename，再同步目录；替换或恢复后的回读/校验失败统一返回
`OutcomeUnknown`，保留当前文件供账本恢复，不能解释为未写入。写前错误分类不变。首次设置与敏感配置复用
application 会话凭据校验，拒绝生成不可登录的配置。
runtime 的保存验证和启动使用同一 listener、HTTP 安全/规范 Origin、metrics、session、tracing 投影及部署校验。
Admin 装配传入 OTLP headers 是否存在的非敏感上下文；仍有 headers 时不能保存关闭 tracing 的候选，须先移除敏感字段并重启。
验证只覆盖当前部署/环境快照，不承诺未来外部配置变更仍可启动。
`AdminRuntimeMailDeliveryBudget` 是 Worker 启动与新 runtime 候选共用的跨组预算谓词；不自动放大请求时限，
不把新规则追溯用于已接受命令的恢复。Worker 引用在 Admin 启动时读取，修改引用的部署参数后须重启以更新快照。
`session_signing_key` 从未生效，已移除 accessor 和内部密钥容器；旧字段仅用于返回明确迁移错误，须从敏感 TOML 删除，
既有不透明会话令牌协议不变。

共享 runtime 只含非敏感候选，保存后须重启受影响进程。Admin 的 listener、metrics/tracing 等按 runtime 覆盖；
Public/Worker 的 metrics 来自公共文件，tracing 来自各自角色文件，OTLP headers 始终只在角色敏感 TOML 中。

`equipment_colors.rules` 的类型、种类和品质支持 `categories`、`equipment_types`、`qualities` 多选，
由共享 runtime DTO 原样读写并投影为 `AppConfig.equipment_colors` 窄规则；Public 判定稀有/融合，Worker 保持论坛名称标色。
每维新旧字段不能同时非空，同维 OR、维度间 AND，不展开为多条规则。Public/Worker 启动时冻结规则，保存后须协调重启；
含非空新数组的文件需要支持该契约的三角色与配置运维工具读取，旧版本的未知字段拒绝保持不变。
新种类数组支持十二种护甲材质族及完整部位名称的并集，材质族核对实际分类和首词/部位边界；旧种类单值保持精确匹配。
Public 完整校验 runtime 后仅应用装备规则及注册提示账号，继续使用自身 listener、安全、会话并发上限与公共拍卖模板；不应用 `operations`。
分体及合并部署预检要求三角色 runtime 同源，装备规则与 `bot_players` 名单快照一致，或全部省略；首次设置生成三份相同路径引用。
升级旧配置及重启范围见[运行配置手册](ops/config/README.md#runtime-共享与重启)。

Public/Worker 将 `bot_players` 单独保留为 `AppConfig.bot_players` 窄投影，不持有完整 Admin runtime 文档。
Public 只展示注册收件账号；Worker 仍用名单选择内部履约证据路径，实际完成须通过当前远端身份及资产/资金核验。

`operations` 的 economics、schedule、forum、alerts、worker、mail 六组 `managed` 默认关闭：经济与排期仅影响新拍卖/
后续轮次并须重启 Admin/Worker，其余组重启 Worker；预检始终要求有效模板/排期一致，runtime 同源要求不依赖接管开关，
MoogleMail 凭据、transport、mailbox 身份和全局启用仍归部署配置。浏览器补齐的未接管候选不代表进程当前值。
Runtime world 开关只控制 Worker 候选，与数据库权威 `worlds.enabled` 不同（见
[运行配置契约](docs/contracts/routes.md#runtime-配置与文件事务)）。

`operations.mail.send_intervals` 接管七类 Send 开始间隔，否则继承 `worker.mooglemail.send_intervals`；
每项 0–86400 整数秒、缺省 0，全零不序列化以保持旧 runtime 指纹。0 不取消单账号串行或 HTTP 限速，保存后重启 Worker。
字段、分类与迁移要求见[发送间隔](ops/config/README.md#邮件业务发送间隔)。

`worker.fulfillment.evidence_poll_seconds` 始终由部署文件拥有；即使 `operations.worker.managed=true`，也保留已配置的
COD 领取证据追踪窗口，不回退到默认 300 秒。

## 兼容与脱敏

旧单文件敏感格式必须明确失败；新增字段同步解析、模板与运维说明。启用 Origin allowlist 且未填列表时，仅确定的
TCP listener 可推导规范 Origin，Unix 或未指定 IP 的 listener 必须显式填写。首次排期额外严格校验 `HH:MM`、
IANA 时区与周内顺序，关闭时也校验。HTTP 配置仅含 listener、Origin、速率、Cookie 属性与超时等非敏感策略，
默认忽略转发头（TCP 代理须显式列入可信 CIDR，`trusted_unix_proxy` 只允许有效 Unix listener），Admin 主配置
fallback 与 runtime 最终值分别校验。Operations alert 仅在 PostgreSQL 完整档通过安全水位生成持久化批次，
通知不含原始错误或凭据。
`request_timeout_seconds` 同时限制 HTTP 请求处理和 WebSocket 握手，默认 15 秒；升级完成后的长连接生命周期
由 realtime owner 管理，不沿用该请求处理时限。

回环 HTTP origin 的 IPv6 文本使用 URL 规范形式 `[::1]`；首次设置、论坛 endpoint/Public 链接及 MoogleMail
均接受该回环地址，不扩大各自既有 IPv4 范围或允许非回环 HTTP。

敏感值与配置 `Debug` 固定脱敏，TOML 解析错误只报告安全路径/分类、不回显原始行；调用方只能通过显式 getter
取得必要值，不得写入日志、错误、普通 DTO 或快照。启用 tracing 时拒绝上游 SDK 的环境导出头（见
[文件职责](ops/config/README.md#文件职责)）；诊断归档属于部署配置，原始数据只能按
[邮件诊断规则](ops/mail/diagnostics.md)加密保存。
OTLP 头按 exporter 的同一规则严格百分号解码后，检查名称、禁止覆盖字段、大小写无关重复键、
UTF-8、控制字符及值的 1024 字节上限；原始列表仍限 8192 字节和 32 项。无编码值只借用原文，
不为预检建立字段列表或复制值；非法编码在配置预检阶段失败，不延迟到 exporter 启动。

## 维护与验证

```bash
cargo check --locked -p auction-config
```

仅静态编译；默认检查与测试范围见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。
字段、路由或部署行为变化时同步对应契约和配置模板。
