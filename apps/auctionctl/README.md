# auctionctl

[文档导航](docs/README.md) · [部署](ops/README.md) · [备份与恢复](ops/deploy/RECOVERY.md) · [v28 接管](ops/deploy/LEGACY_V28_ADOPTION.md)

配置诊断、迁移、systemd 总控、备份、恢复及显式维护命令。参数以[CLI 定义](apps/auctionctl/src/bootstrap/runtime/cli.rs)
和 `auctionctl <command> --help` 为准；示例须替换为实际路径、目标和稳定 UUID。仅支持 Linux x86_64 原生目标，
装配入口在编译期拒绝其他 OS/架构，浏览器 WASM 与发布 ABI 见[开发指南](docs/guide/development.md#工具与依赖来源)。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/bootstrap/runtime/cli.rs](apps/auctionctl/src/bootstrap/runtime/cli.rs) | 命令、参数、默认值与确认短语 |
| [src/bootstrap/runtime/dispatch.rs](apps/auctionctl/src/bootstrap/runtime/dispatch.rs) | 分派、角色配置选择与退出码 |
| [src/bootstrap/deployment_control](apps/auctionctl/src/bootstrap/deployment_control) | systemd 顺序、探针、锁与回执 |
| [src/bootstrap/postgres_rebuild](apps/auctionctl/src/bootstrap/postgres_rebuild) | 独立破坏性重建 |
| [src/bootstrap/postgres_restore](apps/auctionctl/src/bootstrap/postgres_restore) | 单事务恢复与后置证明 |
| [src/bootstrap/postgres_legacy_v28](apps/auctionctl/src/bootstrap/postgres_legacy_v28) | v28 专用备份与接管 |
| [src/bootstrap/runtime/legacy_settlement](apps/auctionctl/src/bootstrap/runtime/legacy_settlement) | 旧 Bot 自寄和实收责任的单目标预览、真实证据与显式修复 |

## 职责与实现边界

CLI 只装配配置、应用/存储端口和受控外部工具，不能绕过领域规则、幂等账本或追加式审计。
普通启动与迁移不会隐式执行备份、恢复、DROP/CREATE 或 retention 删除。

## 实现入口

`src/main.rs` 为薄入口，`bootstrap/runtime.rs` 保留装配出口。`runtime/postgres/commands/` 按数据库引导、凭据、
规则、论坛、结算、预检和备份分工（`lot_watch_pm.rs` 只装配安全列表/持久只读核对请求）；`runtime/postgres/artifacts/`
负责 PostgreSQL 环境、文件、manifest/hash、子进程和 stdin 输入，其 `filesystem/parents.rs` 验证灾备文件目录链与属主；
`runtime/managed.rs` 拥有合并部署迁移锁，`runtime/mail_diagnostics/` 拥有邮件证据命令。

当前格式/v28 备份 manifest 与 systemd 启动回执均在同一已打开描述符上校验普通文件及权限，
并以既有 64 KiB 上限加一个检测字节限量读取；规则导入同样按既有正文上限加单个 UTF-8 BOM 限量读取。
这些读取拒绝符号链接和特殊文件，文件并发增长也不能绕过内存预算；不把 metadata 的读取前长度当作最终上限。

## 命令示例

| 任务 | 入口 | 操作说明 |
| --- | --- | --- |
| 配置诊断、部署检查 | `check-config`、`deployment-check` | 本页 |
| 分体启停、迁移、首次建库 | `deployment`、`migrate`、`postgres-bootstrap` | 本页及部署手册 |
| 合并部署迁移 | `migrate-managed` | [server 手册](apps/auction-server/README.md#生命周期与维护) |
| 邮件证据 | `mail-evidence export/inspect-sent/backfill-display` | [诊断手册](ops/mail/diagnostics.md) |
| 凭据和规则导入 | `bootstrap-mail-credentials`、`import-rules-directory` | 本页 |
| 论坛与结算核验 | `forum-write-recovery-list`、`authorize-forum-write-retry`、`verify-legacy-settlement` | 本页 |
| 旧草稿软截止修正 | `repair-draft-soft-close` | [默认只读、显式确认写入](#草稿软截止显式修正) |
| 装备 PM 运维 | `lot-watch-pm list/reconcile` | [仅请求只读对账](#装备-pm-只读对账) |
| Bot 资金 | `bot-funds` | [默认零写预览、显式真实对账](#bot资金对账) |
| 旧 Bot 结算修复 | `legacy-settlement self-send/adopt-collected` | [单目标、默认只读及真实证据](#旧-bot-结算修复) |
| 保留盘点与恢复预检 | `retention-inventory`、`recovery-preflight` | 本页及恢复手册 |
| 当前格式备份/恢复/重建 | `postgres-backup`、`postgres-backup-verify`、`postgres-restore`、`postgres-rebuild` | [恢复手册](ops/deploy/RECOVERY.md) |
| 历史 v28 | `postgres-legacy-v28-backup`、`postgres-legacy-v28-backup-verify`、`postgres-adopt-legacy-v28` | [接管手册](ops/deploy/LEGACY_V28_ADOPTION.md) |

## 配置诊断与退出结果

```bash
AUCTION_CONFIG_FILE=/etc/auction/public.toml /opt/auction/bin/auctionctl check-config public
/opt/auction/bin/auctionctl deployment-check --config-dir /etc/auction --strict-permissions
```

`check-config` 接受 `public`、`admin`、`worker`，输出 JSON 安全投影：后端、监听/非敏感路径、缓存、tracing/metrics、
安全计数和有效 Worker/runtime 策略；不连接数据库，也不输出连接串、Cookie、数据密钥、OTLP headers 或 `.keyenv` 正文。

## 邮件证据命令

| 子命令 | 访问范围 | 结果边界 |
| --- | --- | --- |
| `export <UUID> --output <new-path> [--key-source credential\|equipment]` | 本地 Worker 配置与加密诊断目录；不联网 | 以 `create_new`/0600 导出 JSON，不向 stdout 输出正文 |
| `inspect-sent <obligation-id> --output <new-path>` | PostgreSQL 和远端 Sent 只读查询 | 仅 `withdrawn_return` 单件装备、零 Credits/COD；不发送或改变履约状态 |
| `backfill-display <evidence-id> [--key-source credential\|equipment]` | 本地认证证据和 PostgreSQL 写入 | 精确匹配 world/mailbox/mid/digest 后，以 CAS、幂等和审计补历史展示 |

父密钥来源省略时优先 credential、缺失才用 equipment，历史导出须保留对应旧密钥备份。Sent 查询要求唯一精确匹配
并标记 `time_unverified`，`collected` 不是已送达证明，它是联网只读诊断而非离线校验或自动 reconcile；
回填仅接受 `parsed_message`、不覆盖已有展示。示例见[邮件诊断](ops/mail/diagnostics.md)。
`inspect-sent` 报告复用私有证据写入器：输出必须位于规范私有目录，采用 `create_new`/0600，完成文件和父目录 fsync
后才报告成功；不覆盖既有证据，不导出凭据。

## Bot资金对账

[runtime/bot_funds](apps/auctionctl/src/bootstrap/runtime/bot_funds) 的 `bot-funds --expected-uid <UID>` 默认仅数据库只读责任预览，
输出版本、完整安全责任投影、blockers 和 SHA-256 摘要；不取得 owner、不观察远端、不声称可用额度。
`--execute` 才装配既有 MM/论坛网关，要求数据库 active 凭据及真实联合身份、Main 现金、clean compose 和 generation，
UID 参数仅断言目标。必须提供 `--command-id`、`--actor`、`--expected-version`、`--preview-digest`、
`--proprietary-credits`、`--compensation-credits`、`--dedicated-account-no-external-liabilities`，及逐字
`--confirm-write 'RECONCILE BOT FUNDS <UID> <VERSION> <PROPRIETARY> <COMPENSATION> <DIGEST>'`。
不默认分配总余额；原命令先重放，版本/责任变化拒绝，未知提交按原 ID 核对。

操作前须停止旧 Worker、所有相关程序同版升级、专用账号无并发资金写入且无未登记外部负债；旧 unknown 或责任
无法完整界定即阻断，账实缺口冻结新增支出并以退出码 5 报告。apply 还要求已启用论坛读取和数据库 active
Main/shared 邮件、论坛凭据，同版 CLI 取得原 advisory owner 后推进 generation 并复用单一 MM session 完成读取
（preview 不走此装配），PostgreSQL 构建从 delivery owner 唯一查询 V1 能力。未资金化 COD 已外发但收款未闭合、
或付款关联 delivery 缺失时，preview/apply 都以 `lot:<lot_id>:payment_collection_unclosed` 阻断 opening。
[升级门禁](ops/README.md#bot-内部履约与资金保护升级)包含顺序迁移至 `000048` 和全 writer 停机。

## Bot只记账开户

独立 `bot-funds-record-only` 为尚未开户的 UID 建立持久化 `record_only` 策略；已有严格资金账户不能用此命令切换。
旧 `bot-funds` 的参数、确认短语及幂等摘要保持兼容。只记账模式允许人工消费 Credits，系统保留结算责任、
真实收款与投递结果的账本，不以远端现金数量核对或限制个人消费。

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml /opt/auction/bin/auctionctl bot-funds-record-only --expected-uid <UID>
```

默认仅数据库预览，输出账户版本、责任摘要与 `confirm_write`，不取得 owner 或访问远端。执行追加
`--execute --command-id <UUID> --actor <ACTOR> --expected-version 0 --preview-digest <HEX>`，以及逐字
`--confirm-write 'INITIALIZE RECORD ONLY BOT FUNDS <UID> 0 <DIGEST>'`；不接收余额、用途额度或专用账号声明。
须先停止 Worker，让同版 CLI 取得原 mail/forum owner；以真实联合身份、干净 compose 和 generation 的 opaque guard
提交开户，全程不读取 Balance、不寄信、不领取或清理草稿。版本与完整摘要在同一事务重核，账户策略与命令、审计一起保存。
`committed/replayed` 表示完成；已提交命令按原 ID 与全部参数重放，`outcome_unknown` 同样保留原参数核对，不能换 ID 猜测重试。

执行证据时仅将安全闭集阶段的 JSON 日志写入 stderr，stdout 保留命令 JSON；纯预览不初始化日志，也不启用 OTLP。

## 旧 Bot 结算修复

`legacy-settlement self-send --obligation-id <UUID> --expected-uid <UID>` 与
`legacy-settlement adopt-collected --obligation-id <UUID> --expected-uid <UID>` 默认仅数据库预览，输出
`expected_state_version`、`expected_digest`、`confirm_write` 与安全目标，不取得 owner 或访问远端。

执行在同一命令追加 `--execute --command-id <UUID> --actor <ACTOR> --expected-state-version <VERSION>
--expected-digest <HEX> --confirm-write '<预览原文>'`；self-send 另要求 `--reason <原因>`。
确认短语分别为 `REPAIR LEGACY SELF SEND <OBLIGATION> <UID> <VERSION> <DIGEST>` 与
`CLAIM LEGACY BOT SELLER <OBLIGATION> <UID> <VERSION> <DIGEST>`，逐字使用预览值。
CLI 共用三十秒绝对预算和原 mail/forum owner，真实身份、库存或精确原 Sent 与现金在 opaque guard 内借用至提交结束；
Worker 必须先停止。命令不寄信、不领取、不清理草稿，原发送、错误、金额和命令历史保留。

self-send 对已核实自持的流拍装备追加留存事实；成交自寄明确拒绝的核验仅恢复受同 UID 约束的内部付款准备，
不表示买家已付款。adopt-collected 从已隔离责任转为旧 Bot 卖家留存，不再次发行旧收款或授予自有额度。
执行输出 `committed/replayed` 才表示完成；`rolled_back` 为确定回滚，`evidence_unavailable` 为提交前证据未通过，
`outcome_unknown` 须保留原命令和全部参数核对，`funding_deferred` 表示资金门已提交冻结或保留未知占用且认领未完成。
开户、旧责任认领及恢复同版 Worker 的完整顺序见[运维流程](ops/README.md#旧-bot-结算的显式核对)。

## 稳定退出码

| 退出码 | 含义 |
| --- | --- |
| `0` | 成功 |
| `2` | 参数解析，或 check-config/deployment-check 失败 |
| `3` | 迁移、初始建库、盘点、备份/恢复/重建/v28 接管命令失败 |
| `4` | deployment 命令失败 |
| `5` | 凭据/规则导入、论坛/结算核验、邮件证据、草稿软截止修正、Bot资金对账/只记账开户命令失败或已提交冻结 |

退出码按命令类别分配，不按底层错误类型重新分类；调用方不应解析脱敏错误文本。
显式 `--help`（包括子命令帮助）写入 stdout 并返回 `0`，不加载配置或执行命令；非法参数仍返回 `2`。

## 部署预检

`deployment-check` 只读三角色及其公共、runtime、敏感文件，核对同一公共文件/数据库目标、listener 冲突、
Admin/Worker control socket、三组数据密钥和有效拍卖/结算模板与排期；三角色必须引用同一 runtime 文件且装备规则快照一致，
或全部省略 runtime。默认报告权限警告，生产使用 `--strict-permissions`，配置目录和文件均不得是
符号链接；命令不连接数据库、迁移、启动服务或访问外部服务。目标比较复用 SQLx 解析并允许不同数据库账号，
PostgreSQL 显式提供 host、非零 port 和库名，允许受支持的 host/hostaddr/port/dbname 覆盖及 Unix socket，
拒绝未知参数和 `options`，DNS 别名不解析，IPv6 URI 方括号在实际 TCP/libpq host 中去除（见[配置手册](ops/config/README.md)）。
严格模式同时要求 runtime 文件精确 `0640`，父目录为非符号链接且禁止 group/other 写入。分体启动/重启在迁移前读取
实际 Admin unit 的 `User`（空值按 systemd root 语义），要求 runtime 文件和可写父目录归该固定 UID；
不能用运维调用者 UID 代替服务身份，使用 runtime 时拒绝 `DynamicUser=yes` 并要求先调整部署。

## Systemd 总控

`deployment start/restart/stop/status` 只用于 Linux/systemd 分体部署。三角色均停止时，start 严格预检、迁移，
再按 Worker → Admin → Public 逐个等待 readiness；全部已运行时仅复核顺序并发布 adopted 回执，部分运行或转换中
则拒绝。restart 先预检，再按 Public → Admin → Worker 停止并重复启动流程；失败只停止本次启动的 unit，
不回退已提交迁移。总控默认使用 `/usr/bin/systemctl`、`/var/lib/auction/deployment` 和三个 `auction-*.service`
unit，由 `--systemctl`、`--state-dir`、`--worker-unit`、`--admin-unit`、`--public-unit` 覆盖；start/restart
默认配置目录 `/etc/auction`，每角色 readiness 预算 30 秒（1–300 秒可调）。锁、unit 环境绑定、1 秒单次探针
与回执见[systemd 手册](ops/README.md#systemd-服务与顺序控制)。

## 停机迁移与初始建库

```bash
AUCTION_CONFIG_FILE=/etc/auction/public.toml /opt/auction/bin/auctionctl postgres-bootstrap \
  --confirm-database auction_production --maintenance-secrets-file /etc/auction/postgres-rebuild.secret.toml
AUCTION_CONFIG_FILE=/etc/auction/public.toml /opt/auction/bin/auctionctl migrate
```

两条命令均使用同一部署状态锁与 unit 参数。`migrate` 要求三角色严格停止，拒绝运行、转换状态或状态读取失败，
只有空库或合法 SQLx 历史才能迁移，不接管未知用户对象；`deployment start/restart` 在其已持有的锁与停机边界内
调用迁移。`postgres-bootstrap` 加载 Public 配置，确认名须与目标 URL path 一致，允许 unit 为 `not-found` 但仍
要求明确无进程；取得同 rebuild 的集群 advisory lock 后检查可写主库、维护角色 CREATEDB、应用角色及 owner 分配
权限，拒绝系统库和不安全库名。目标缺失才 CREATE DATABASE，存在则只校验 owner 和应用连接身份并以
`already_exists` 幂等成功，绝不修改已有库或执行迁移（见[配置说明](ops/config/README.md#postgresql-维护凭据)）。

合并部署先停止整个 `auction-server`，以相同服务账号执行 `auctionctl migrate-managed --config-dir <配置目录>`：
它要求既有 `owner.lock` 并取得 supervisor/子进程共享锁的独占所有权，普通 migrate 与分体总控也复查此锁，
不能在同库混用合并和分体部署，失败使用退出码 3。

## Cargo feature

默认不带驱动。`postgres` 启用数据库驱动与迁移；配置了数据库但未编译驱动时，迁移/启动在启动 unit 前明确失败。
`cache-redis` 只在规则导入需要 Redis 失效时使用。

## 凭据与规则导入

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml /opt/auction/bin/auctionctl bootstrap-mail-credentials \
  --world main --label initial-mail-cookie < /secure/auction/mail-cookie
AUCTION_CONFIG_FILE=/etc/auction/admin.toml /opt/auction/bin/auctionctl import-rules-directory \
  --directory /secure/auction-migration/rules --command-id <稳定命令UUID> --actor rules-import
```

凭据命令仅接受 Worker 已启用的 `main`/`isekai`，用敏感配置的独立 credential 数据密钥加密写入 PostgreSQL；
Cookie 只经 stdin，来源文件须 owner-only，导入后按 runtime 要求重启 Worker。规则命令加载 Admin 配置，须提供
`en.md`、`zh-CN.md`、`zh-TW.md`、`ja.md`、`ko.md` 五个 UTF-8 普通非符号链接文件：移除单个开头 BOM，首个非空行
去掉前导 `#` 作为标题。已有相同内容报告 `preserved`，任何不同内容在首个写入前拒绝，缺失语言才发布；各语言 UUID
由批次 ID 稳定派生，部分完成后用同一批次重试。输出 `sha256` 针对去 BOM 后正文；Redis 配置要求
`postgres,cache-redis`，缓存失效失败列入 `cache_invalidation_failures` 且不回滚已提交规则。
规则正文读取后原地移除 BOM，并按所有权移交候选与发布命令，避免复制整篇 Markdown；
大小限制、规范化、SHA-256、稳定命令 ID 与逐语言提交语义保持不变。

## 草稿软截止显式修正

`repair-draft-soft-close` 加载 Admin 配置并需要 `postgres` feature。默认以 Serializable READ ONLY 事务检查唯一目标，
输出 `eligible`、`blockers`、当前版本和两项秒数；检查成功但不合格时仍返回 JSON，操作者必须阅读 `eligible`。
只读检查不生成命令账本，也不锁定后续执行资格。

```bash
AUCTION_CONFIG_FILE=/etc/auction/admin.toml /opt/auction/bin/auctionctl repair-draft-soft-close \
  --auction-id <目标UUID> --execute --expected-version <检查版本> --command-id <稳定命令UUID> \
  --actor <操作者> --confirm-write 'REPAIR DRAFT SOFT CLOSE <目标UUID>'
```

先按[有效模板升级步骤](ops/config/README.md#缓存与拍卖模板)更新后续新场配置并受控重启，人工核对检查结果后保留
一次生成的 command UUID 及完整原参数再执行；占位符须替换为实际值，确认文本 UUID 必须为规范小写连字符形式。
命令只允许当前 draft、从未预览/开拍、无竞价或代理授权、无结算、无论坛任务及外部写入/未知发布证据的目标，
缺少排期记录或仍有排期租约同样阻断，允许保留未公开投稿和 pending/withdrawn 装备；行与策略快照必须一致，
窗口/延长均为 60 秒，命令固定改成 900/900，保留其他经济参数、装备、资金和排期。执行使用 world →
scheduled round → auction 行锁、版本 CAS 和 Serializable，40001/40P01 最多三次，两项列值、JSON 字段、版本及
更新时间与原始结果账本、追加审计同事务提交（`auction_commands`，审计动作 `auction.repair_draft_soft_close`）。
同 ID/同参数先重放原结果，异参数冲突，超时/断线保留原命令重试；该命令不启动服务、发帖、发信或重开旧场次。
实现由 [CLI 装配](apps/auctionctl/src/bootstrap/runtime/postgres/commands/draft_soft_close.rs)、
[应用规则/窄端口](crates/auction-application/src/draft_soft_close)和
[存储事务](crates/auction-storage/src/database/draft_soft_close)拥有。

## 装备 PM 只读对账

`lot-watch-pm list --status manual_reconcile --limit 50 --offset 0` 只查询安全状态、fence、时间及原 sender，
不暴露正文、收件人、租约 token 或原始响应（limit 1–100，offset 0–10000）；
`lot-watch-pm reconcile --notification-id <uuid> --command-id <uuid> --expected-fencing-version <positive-i64> --actor <operator>`
以稳定命令、版本 CAS、追加审计请求 Worker 只读核对：成功返回原接受快照但不表示已寄出，未知请求原负载重试。
只允许无有效核对租约的 manual_reconcile，不提供重发、abandoned、强制成功或 raw external-reference 动作；
CLI 只装配数据库应用端口，不加载第二个论坛客户端。需要 `postgres` feature，门禁与 `000042` 升级见
[部署手册](ops/README.md#装备标记与超价提醒升级)。

## 论坛与结算核验

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml /opt/auction/bin/auctionctl forum-write-recovery-list --limit 100
AUCTION_CONFIG_FILE=/etc/auction/worker.toml /opt/auction/bin/auctionctl authorize-forum-write-retry \
  --auction-id <拍卖UUID> --command-id <稳定命令UUID> --actor <操作者> \
  --confirm-write 'AUTHORIZE FORUM WRITE <拍卖UUID>'
AUCTION_CONFIG_FILE=/etc/auction/worker.toml /opt/auction/bin/auctionctl verify-legacy-settlement --auction-id <拍卖UUID>
```

这些命令仅支持 PostgreSQL。论坛 list 只读数据库；Worker 先按精确标题 GET 对账，未命中停在
`write_authorization_required` 后，retry 命令才可用非零 UUID、规范 actor、逐字确认、幂等账本和审计授权下一次 POST，
相同 command ID 不同摘要冲突。已存在主题但固定帖正文校验触发门禁时，应先核对 #1/#2 标记和真实帖子身份，
更新程序/部署配置后再用同一入口（Worker 先查找现有主题、核对或补写固定帖，不重开主题）；更新和重启不会自动
清空该人工门禁，禁止直接改状态或重试时间伪造发布成功。主题同步历史 `write_unknown`/`publish_proof_failed`
（含 `reconcile_` 前缀）可在新 Worker 首轮水位下重新只读对账一次，启用 `worker.forum.verify_post_body` 时
原始正文匹配才自动收敛，关闭时转人工授权。对账不发 POST，本组 CLI 也不处理已发布主题的
`content_last_error_code=write_unknown`。结算核验在 `SERIALIZABLE READ ONLY DEFERRABLE` 快照内重建旧
`settlement_complete/OK` 摘要与当前权威事实，缺失、身份漂移或损坏即失败，不补 completion fence、不重新结算。

## 保留盘点与恢复预检

`retention-inventory --confirm-database <name> --output <new-path>` 只读 schema、迁移、关系大小、行数估计、外键和
realtime 水位，并保存[政策](ops/deploy/retention-policy.v1.json) SHA-256；`eligibility_assessed=false`、
`eligibility_authorized=false` 固定不变，不能据此归档或删除。`recovery-preflight` 使用同形参数，只核验目标身份、
可写主节点、其他连接和空库/迁移条件，`restore_preflight_passed` 不代表 archive 已验证，也不会停止连接或建库。
两者均创建不可覆盖 0600 证据（示例见[数据库恢复与保留](ops/README.md#数据库恢复与保留)）。

## 备份与历史接管

当前 schema 用 `postgres-backup` / `postgres-backup-verify` 产生 custom-format archive 和配对 manifest
（外部 `pg_dump`/`pg_restore` 由部署环境提供），verify 离线校验身份、SHA-256 与 archive 目录。canonical v28
使用独立备份/verify/adoption：默认只读，执行需专用恢复点、`--execute`、逐字 `ADOPT LEGACY V28 <数据库名>`
和新 receipt，普通 backup/restore 不接受 v28 manifest，bridge 或接管已提交后失败不能再次 adoption。
步骤见[恢复手册](ops/deploy/RECOVERY.md)与[v28 手册](ops/deploy/LEGACY_V28_ADOPTION.md)。

## 恢复与破坏性重建

`postgres-restore` 只向匹配 manifest 身份的同名空库恢复，要求零外部连接、可写主节点和无用户对象；
`--dry-run` 只读且不得提供 receipt，实际执行使用单事务 `pg_restore`，schema 后验通过后发布 receipt。
提交后校验或回执失败不等于数据库未变化，须保持停机；旧版恢复也核对完整 SQLx 历史前缀、状态及 checksum。
`postgres-rebuild` 默认只读，执行必须同时有精确目标、`--execute`、`REBUILD <数据库名>`、新 receipt 及
verified backup；只有明确可丢弃的一次性数据库可改用 `--allow-unrecoverable-ephemeral` 与
`EPHEMERAL <数据库名>`。`--mode empty-for-restore` 强制备份且只准备空库，`fresh-schema` 在空库上执行当前迁移；
未知结果不能推断为未执行，也不能盲目重试 DROP。

## 安全约束

连接串、Cookie、密钥不进入参数、日志或回执。灾备文件使用受信私有目录、新路径与不可覆盖发布，文件和父目录
同步后才报告成功，mutation 前也同步恢复证据，不支持同步时失败关闭。破坏性操作必须按独立手册精确确认目标和
恢复点，不加入普通启动、迁移、unit 或开发脚本；未知结果不能推断为未执行，也不能盲目重试 DROP。

## 维护与验证

`cargo check --locked -p auctionctl` 只做默认 feature 静态编译，完整门禁见[开发指南](docs/guide/development.md)；
静态检查不能证明备份可恢复或外部操作成功，运行证据见[验证状态](docs/status.md)。
