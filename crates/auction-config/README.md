# auction-config

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

公共、角色、敏感和 runtime TOML 的解析、校验及安全文件适配。

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
| [src/config](src/config) | 配置加载、模型、默认值与角色约束 |
| [src/runtime](src/runtime) | Admin runtime 编解码、文件锁、候选校验与原子替换 |
| [src/scheduling](src/scheduling) | Worker cron、时区与下一次触发时刻 |
| [src/setup](src/setup) | 首次配置物化、权限、原子发布与三角色校验 |

## 职责与实现边界

本 crate 拥有部署格式与配置 I/O，向应用层提供已校验的值；业务模块不得自行读取文件或环境变量。
完整字段、默认值、单位与启用条件统一维护在[配置手册](../../ops/config/README.md)及其模板中。

## 解析与装配入口

`config/settings/` 按 `auction`、`worker`、`secrets`、`app`、`validation` 和 `operations` 分工。
角色文件先定位为绝对路径，相对公共/敏感/runtime 引用均以其目录解析；该步骤不解除 symlink 安全检查。
`AuctionConfig::policy()` 返回 `Result`，不可表示的时间范围应返回配置错误，不 panic 或截断。
新场次 soft-close 窗口/延长默认均为 900 秒；common、runtime 候选和首次设置所内嵌模板同步。
升级不覆盖文件中显式的 60；须核对 managed 的有效来源并手动更新，步骤见[配置手册](../../ops/config/README.md#缓存与拍卖模板)。

`FirstRunConfig::write_to` 只向明确不存在的目标发布。私有临时目录中生成文件、固定非敏感 `0640`/敏感 `0600`，
复用三角色 loader 与正式 runtime 校验后同步目录，以 Linux `RENAME_NOREPLACE` 发布并同步父目录。
发布前仅清理本次临时目录；发布后确认失败返回 `OutcomeUnknown` 并保留目标，不回滚。父目录必须受信。
`write_managed_to` 只额外推导配置目录同级的私有 control 目录，不合并敏感配置。

## 公共、角色、敏感与 runtime 文件结构

三个角色必须通过 `common_file` 引用同一公共非敏感 TOML，各自通过 `secrets_file` 引用敏感 TOML；
Admin/Worker 可共用 `runtime_config_file`。文件按 owner 分别加载，禁止拼成敏感单文件。

模板入口：[common](../../ops/config/examples/common.toml)、[Public](../../ops/config/examples/public.toml)、
[Admin](../../ops/config/examples/admin.toml)、[Worker](../../ops/config/examples/worker.toml)、
[runtime](../../ops/config/examples/runtime/admin.toml)。Public 会话默认 idle/absolute 为 1800/604800 秒；
Admin runtime 对应独立默认值为 900/28800 秒，不能混用。

## 敏感字段与数据密钥

[settings/secrets](src/config/settings/secrets) 严格定义敏感字段，不接受任意键值透传。
PostgreSQL `[database]` 接收 host、port、username、password、name，统一校验并编码为 URL；与兼容 `database_url` 互斥。
需要 Unix socket 或显式 TLS profile 时使用兼容 URL。端口必须非零，输入与 `Debug` 均不得泄密。

Admin actor/secret 必须成对，secret 至少 16 字节。装备、外部凭据、用户登录 key 各自使用独立的
数据密钥与非秘密 key ID，密钥为 32 字节标准 Base64，不得互相复用。
Public 的受限装备链接能力需要匹配装备解密密钥；A32 凭据管理需要 PostgreSQL 与独立 credential key。
敏感文件禁止提交，访问权限限运行账号，字段及跨角色一致性要求见[敏感配置](../../ops/config/README.md#敏感字段)。

## PostgreSQL 维护敏感配置

`load_postgres_maintenance_secrets` 为 bootstrap/rebuild 独立加载指向 `postgres` 维护库的 TOML。
要求规范绝对路径、普通非 symlink 文件和安全父目录；Unix 下须归当前有效用户、单 hard link、无 group/other 权限。
维护 URL 仅经显式 getter 短暂使用，不进入命令参数、日志、receipt 或 DTO；见[维护凭据](../../ops/config/README.md#postgresql-维护凭据)。

## MoogleMail 本地联调凭据

`load_mooglemail_keyenv` 只接受 `.keyenv` 的 `hv-ck-key1` 与 `hv-ck-key2`，拒绝未知/重复键、空值、控制字符和不安全权限。
Worker 只使用 key1；key2 仅供显式隔离验证，不能作为候选或自动回退。该文件不是通用环境覆盖，禁止提交；
生产部署使用敏感 TOML 或数据库密文凭据。

## Worker 控制与参数校验

配置层校验 control 路径、正数范围、lease/续租/单项 timeout 关系及后端能力。
Auction 默认并发为 8。
Delivery 有效并发取配置与 transport 上限的较小值；MoogleMail transport 当前为 1。
启用 MoogleMail 时发送总预算必须大于两倍读取预算，sending grace 必须大于发送预算。
定时器只唤醒持久化任务，不能代替 due、租约或游标；具体值见[Worker 预算](../../ops/config/README.md#worker-循环预算)。
croner 4.0.1 前仍先做严格六字段语法检查；不开放上游新增昵称/宽松输入，不改秒字段和时区契约。

## Runtime 文件锁

Admin runtime 更新/恢复使用非阻塞 `File::try_lock()`，从恢复检查持续持有至文件与账本收口。
竞争返回不可用，guard 取消后由 RAII 释放；锁文件不逐次删除，流程内复用已持锁接口，避免锁域分裂或递归取锁。
Unix runtime 文件须由有效 UID 拥有且为 0640；首次设置与敏感配置复用 application 会话凭据校验，拒绝生成不可登录的配置。

## 告警与 HTTP 安全

HTTP 配置仅含 listener、Origin、速率、Cookie 属性与超时等非敏感策略。默认忽略转发头；
TCP 代理须显式列入可信 CIDR，`trusted_unix_proxy` 只允许有效 Unix listener。
Admin 主配置 fallback 与 runtime 最终值分别校验。
Operations alert 仅在 PostgreSQL 完整档通过安全水位生成持久化批次，通知不含原始错误或凭据。

## 邮件轮询与预算

每个 world 使用独立 mailbox、轮询状态与 cron。`batch_timeout_seconds` 默认 600，范围 1–86400，
不得小于 fetch 与 item 预算之和；覆盖拉取、重试与业务处理，独立于可续期 lease。
预算耗尽只能提交已完成连续前缀，未完成邮件不能被 cursor 跳过；单次 fetch 也可返回已验证的最旧详情前缀。

论坛、邮件、发件人 allowlist、装备过滤及诊断归档的完整配置见[论坛与邮件](../../ops/config/README.md#论坛与邮件)。
`worker.forum.verify_post_body` 是默认关闭的部署侧正文校验开关，不属于 runtime 的 `operations.forum` 接管范围。
诊断目录/保留预算属于部署配置，runtime 不接管；原始数据只能按[邮件诊断规则](../../ops/mail-diagnostics.md)加密保存。

## Runtime 生效范围

Admin/Worker 共享的 runtime 只含非敏感候选，保存后须重启受影响进程。
Admin 的 listener、metrics/tracing 等按 runtime 覆盖；Public/Worker 的 metrics 来自公共文件，tracing 来自各自角色文件。
OTLP headers 始终只在角色敏感 TOML 中。

`operations` 的 economics、schedule、forum、alerts、worker、mail 六组 `managed` 默认关闭。
经济与排期仅影响新拍卖/后续轮次，须重启 Admin/Worker；其余组重启 Worker。
接管任一组时预检要求共用同一 runtime，且有效模板/排期一致；MoogleMail 凭据、transport、mailbox 身份和全局启用仍归部署配置。
旧文档省略 operations/mail 时保留原 fingerprint；浏览器补齐的未接管候选不代表进程当前值。

Runtime world 开关只控制 Worker 候选，与数据库权威 `worlds.enabled` 不同；新轮次须取配置 allowlist、runtime 和数据库开关的交集。
文件事务及生效规则见[运行配置契约](../../docs/contracts/routes.md#runtime-配置与文件事务)。

## 环境变量边界

`AUCTION_CONFIG_FILE` 仅选择角色配置文件，不覆盖任何单个字段；业务模块不得自行读取环境变量。
启用 tracing 时拒绝上游 SDK 的环境导出头，边界与处理见[配置手册](../../ops/config/README.md#文件职责)。

## 凭据保护

敏感值与配置 `Debug` 固定脱敏。TOML 解析错误只报告安全路径/分类，不回显原始行。
调用方只能通过显式 getter 取得必要值，不得写入日志、错误、普通 DTO 或快照。

## 兼容性

旧单文件敏感格式必须明确失败。新增字段同步解析、模板与运维说明。
启用 Origin allowlist 且未填列表时，仅确定的 TCP listener 可推导规范 Origin；Unix 或未指定 IP 的 listener 必须显式填写。
首次排期额外严格校验 `HH:MM`、IANA 时区与周内顺序，关闭时也校验，且生成的 runtime 不接管排期。

## 维护与验证

```bash
cargo check --locked -p auction-config
```

仅静态编译；默认检查与测试范围见[开发指南](../../docs/development.md)，运行证据见[验证状态](../../docs/status.md)。
字段、路由或部署行为变化时同步对应契约和配置模板。
