# auctionctl

[文档导航](../../docs/README.md) · [部署](../../ops/deploy/README.md) · [备份与恢复](../../ops/deploy/RECOVERY.md) · [v28 接管](../../ops/deploy/LEGACY_V28_ADOPTION.md)

配置诊断、迁移、systemd 总控、备份、恢复及显式维护命令。参数以
[CLI 定义](src/bootstrap/runtime/cli.rs)和 `auctionctl <command> --help` 为准；命令示例须替换为实际路径、目标和稳定 UUID。

仅支持 Linux x86_64 原生目标；装配入口在编译期拒绝其他 OS/架构。浏览器 WASM 不受此限制，
默认发布 ABI 与构建边界见[开发指南](../../docs/development.md#工具与依赖来源)。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/bootstrap/runtime/cli.rs](src/bootstrap/runtime/cli.rs) | 命令、参数、默认值与确认短语 |
| [src/bootstrap/runtime/dispatch.rs](src/bootstrap/runtime/dispatch.rs) | 分派、角色配置选择与退出码 |
| [src/bootstrap/deployment_control](src/bootstrap/deployment_control) | systemd 顺序、探针、锁与回执 |
| [src/bootstrap/postgres_rebuild](src/bootstrap/postgres_rebuild) | 独立破坏性重建 |
| [src/bootstrap/postgres_restore](src/bootstrap/postgres_restore) | 单事务恢复与后置证明 |
| [src/bootstrap/postgres_legacy_v28](src/bootstrap/postgres_legacy_v28) | v28 专用备份与接管 |

## 职责与实现边界

CLI 只装配配置、应用/存储端口和受控外部工具；不能绕过领域规则、幂等账本或追加式审计。
普通启动与迁移不会隐式执行备份、恢复、DROP/CREATE 或 retention 删除。

## 实现入口

`src/main.rs` 为薄入口，`bootstrap/runtime.rs` 保留装配出口。
`runtime/postgres/commands/` 按数据库引导、凭据、规则、论坛、结算、预检和备份分工；
`runtime/postgres/artifacts/` 负责 PostgreSQL 环境、文件、manifest/hash、子进程和 stdin 输入；
其 `filesystem/parents.rs` 验证灾备文件完整目录链与属主，拒绝非可信账号可替换的路径。
`runtime/managed.rs` 拥有合并部署迁移锁，`runtime/mail_diagnostics/` 拥有邮件证据命令。
其他专用 owner 见上表，不在分派层重复实现其安全门禁。

## 命令示例

| 任务 | 入口 | 操作说明 |
| --- | --- | --- |
| 配置诊断、部署检查 | `check-config`、`deployment-check` | 本页 |
| 分体启停、迁移、首次建库 | `deployment`、`migrate`、`postgres-bootstrap` | 本页及部署手册 |
| 合并部署迁移 | `migrate-managed` | [server 手册](../server/README.md#生命周期与维护) |
| 邮件证据 | `mail-evidence export/inspect-sent/backfill-display` | [诊断手册](../../docs/operations/mail-diagnostics.md) |
| 凭据和规则导入 | `bootstrap-mail-credentials`、`import-rules-directory` | 本页 |
| 论坛与结算核验 | `forum-write-recovery-list`、`authorize-forum-write-retry`、`verify-legacy-settlement` | 本页 |
| 保留盘点与恢复预检 | `retention-inventory`、`recovery-preflight` | 本页及恢复手册 |
| 当前格式备份/恢复/重建 | `postgres-backup`、`postgres-backup-verify`、`postgres-restore`、`postgres-rebuild` | [恢复手册](../../ops/deploy/RECOVERY.md) |
| 历史 v28 | `postgres-legacy-v28-backup`、`postgres-legacy-v28-backup-verify`、`postgres-adopt-legacy-v28` | [接管手册](../../ops/deploy/LEGACY_V28_ADOPTION.md) |

## 配置诊断与退出结果

```bash
AUCTION_CONFIG_FILE=/etc/auction/public.toml /opt/auction/bin/auctionctl check-config public
/opt/auction/bin/auctionctl deployment-check --config-dir /etc/auction --strict-permissions
```

`check-config` 接受 `public`、`admin`、`worker`，输出 JSON 安全投影：后端、监听/非敏感路径、
缓存、tracing/metrics、安全计数和有效 Worker/runtime 策略。它不连接数据库，不输出连接串、Cookie、
签名密钥、OTLP headers 或 `.keyenv` 正文。

## 邮件证据命令

| 子命令 | 访问范围 | 结果边界 |
| --- | --- | --- |
| `export <UUID> --output <new-path> [--key-source credential\|equipment]` | 本地 Worker 配置与加密诊断目录；不联网 | 以 `create_new`/0600 导出 JSON，不向 stdout 输出正文 |
| `inspect-sent <obligation-id> --output <new-path>` | PostgreSQL 和远端 Sent 只读查询 | 仅 `withdrawn_return` 单件装备、零 Credits/COD；不发送或改变履约状态 |
| `backfill-display <evidence-id> [--key-source credential\|equipment]` | 本地认证证据和 PostgreSQL 写入 | 精确匹配 world/mailbox/mid/digest 后，以 CAS、幂等和审计补历史展示 |

父密钥来源省略时优先 credential、缺失才用 equipment；历史导出须保留对应旧密钥备份。
Sent 查询要求唯一精确匹配并标记 `time_unverified`，`collected` 不是充分的已送达证明；
它是联网只读诊断，不是离线校验或自动 reconcile。回填仅接受 `parsed_message`，不覆盖已有展示，
不将诊断内容用作业务事实。完整示例见[邮件诊断](../../docs/operations/mail-diagnostics.md)。

## 稳定退出码

| 退出码 | 含义 |
| --- | --- |
| `0` | 成功 |
| `2` | 参数解析，或 check-config/deployment-check 失败 |
| `3` | 迁移、初始建库、盘点、备份/恢复/重建/v28 接管命令失败 |
| `4` | deployment 命令失败 |
| `5` | 凭据/规则导入、论坛/结算核验、邮件证据命令失败 |

退出码按命令类别分配，不按底层错误类型重新分类；调用方不应解析脱敏错误文本。

## 部署预检

`deployment-check` 只读三角色及其公共、runtime、敏感文件，核对同一公共文件/数据库目标、
TCP/Unix listener 冲突、Admin/Worker control socket、三组数据密钥和有效拍卖/结算模板与排期。
启用 runtime `operations` 任一组时还要求 Admin/Worker 引用同一 runtime 文件。
默认报告权限警告，生产使用 `--strict-permissions`；配置目录和文件均不得是符号链接。
不连接数据库、迁移、启动服务或访问外部服务。

目标比较复用 SQLx 解析，允许不同数据库账号；PostgreSQL 显式提供 host、非零 port 和库名，
允许受支持的 host/hostaddr/port/dbname 覆盖及 Unix socket，拒绝未知参数和 `options`。
DNS 别名不解析，应统一规范主机名；IPv6 URI 方括号在实际 TCP/libpq host 中去除。
具体文件规则见[配置手册](../../ops/config/README.md)。

## Systemd 总控

`deployment start/restart/stop/status` 只用于 Linux/systemd 分体部署。
三角色均停止时，start 严格预检、迁移，再按 Worker → Admin → Public 逐个等待 readiness；
全部已运行时仅复核 readiness/启动顺序并发布 adopted 回执，部分运行或转换中则拒绝。
restart 先预检，再按 Public → Admin → Worker 停止并复核停机后重复启动流程。
失败只停止本次启动的 unit，不回退已提交迁移。

总控默认使用 `/usr/bin/systemctl`、`/var/lib/auction/deployment` 和三个 `auction-*.service` unit；
参数由 `--systemctl`、`--state-dir`、`--worker-unit`、`--admin-unit`、`--public-unit` 覆盖。
start/restart 默认配置目录 `/etc/auction`，每角色 readiness 预算 30 秒，可设为 1–300 秒。
锁、unit 环境绑定、1 秒单次探针及回执规则统一见[systemd 手册](../../ops/deploy/systemd/README.md)。

## 停机迁移与初始建库

```bash
AUCTION_CONFIG_FILE=/etc/auction/public.toml \
  /opt/auction/bin/auctionctl postgres-bootstrap \
    --confirm-database auction_production \
    --maintenance-secrets-file /etc/auction/postgres-rebuild.secret.toml

AUCTION_CONFIG_FILE=/etc/auction/public.toml /opt/auction/bin/auctionctl migrate
```

两条命令均使用同一部署状态锁与 unit 参数。`migrate` 要求三角色严格停止，拒绝运行、转换状态或
状态读取失败；只有空库或合法 SQLx 历史才能迁移，不接管未知用户对象。`deployment start/restart`
在其已持有的锁与停机边界内调用迁移。

`postgres-bootstrap` 加载 Public 配置；确认名须与目标 URL path 一致。它还允许 unit 为 `not-found`，
但仍要求明确无进程；取得同 rebuild 的集群 advisory lock 后，检查可写主库、维护角色 CREATEDB、
应用角色及 owner 分配权限。拒绝系统库和不安全库名。目标缺失才 CREATE DATABASE，存在则只校验 owner 和
应用连接身份，以 `already_exists` 幂等成功；绝不修改已有库或执行迁移。
维护凭据/endpoint/TLS 门禁见[配置说明](../../ops/config/README.md#postgresql-维护凭据)。

合并部署先停止整个 `auction-server`，以相同服务账号执行：

```bash
/opt/auction/bin/auctionctl migrate-managed --config-dir /var/lib/auction/config
```

该命令要求既有 `owner.lock` 并取得 supervisor/子进程共享锁的独占所有权；只有父进程退出不够。
普通 migrate 与分体总控也复查此锁，不能在同库混用合并和分体部署；失败使用退出码 3。

## Cargo feature

默认不带驱动。`postgres` 启用数据库驱动与迁移；配置了数据库但未编译驱动时，迁移/启动在
启动 unit 前明确失败。`cache-redis` 只在规则导入需要 Redis 失效时使用。

## 凭据与规则导入

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl bootstrap-mail-credentials \
    --world main --label initial-mail-cookie < /secure/auction/mail-cookie

AUCTION_CONFIG_FILE=/etc/auction/admin.toml \
  /opt/auction/bin/auctionctl import-rules-directory \
    --directory /secure/auction-migration/rules \
    --command-id 123e4567-e89b-42d3-a456-426614174000 --actor rules-import
```

凭据命令仅接受 Worker 已启用的 `main`/`isekai`，通过敏感配置的独立 credential 数据密钥加密写入
PostgreSQL。Cookie 只经 stdin，来源文件须 owner-only，不进入参数或输出；导入后按 runtime 要求重启 Worker。

规则命令加载 Admin 配置，必须提供 `en.md`、`zh-CN.md`、`zh-TW.md`、`ja.md`、`ko.md` 五个 UTF-8
普通非符号链接文件。移除单个开头 BOM，首个非空行去掉前导 `#` 后作为标题，Markdown 其余字节保留。
先校验全部来源和现有文档：已有相同内容报告 `preserved`，任何不同内容在首个写入前拒绝，缺失语言才发布。
各语言 UUID 由批次 ID 稳定派生，部分完成后使用同一批次重试。输出 `sha256` 针对去 BOM 后正文，
有 BOM 时与原文件哈希不同。Redis 配置要求 `postgres,cache-redis`；缓存失效失败列入
`cache_invalidation_failures`，不回滚已提交规则。

## 论坛与结算核验

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl forum-write-recovery-list --limit 100

AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl authorize-forum-write-retry \
    --auction-id 123e4567-e89b-42d3-a456-426614174000 \
    --command-id 223e4567-e89b-42d3-a456-426614174000 --actor operator \
    --confirm-write 'AUTHORIZE FORUM WRITE 123e4567-e89b-42d3-a456-426614174000'

AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl verify-legacy-settlement \
    --auction-id 123e4567-e89b-42d3-a456-426614174000
```

这些命令仅支持 PostgreSQL。论坛 list 只读数据库；Worker 先按精确标题 GET 对账，未命中停在
`write_authorization_required` 后，retry 命令才可用非零 UUID、规范 actor、逐字确认、幂等账本和审计授权下一次 POST。
相同 command ID 不同摘要冲突，不能绕过对账盲目重发。

结算核验在 `SERIALIZABLE READ ONLY DEFERRABLE` 快照内重建旧 `settlement_complete/OK` 摘要与
当前权威事实，缺失、身份漂移或损坏即失败。它不补 completion fence、不重新结算、不追加任何事实。

## 保留盘点与恢复预检

`retention-inventory --confirm-database <name> --output <new-path>` 只读 schema、迁移、关系大小、
行数估计、外键和 realtime 水位，并保存[政策](../../ops/deploy/retention-policy.v1.json) SHA-256。
`eligibility_assessed=false`、`eligibility_authorized=false` 固定不变，不能据此归档或删除。

`recovery-preflight` 使用同形参数，只核验目标身份、可写主节点、其他连接和空库/迁移条件。
`restore_preflight_passed` 不代表 archive 已验证；命令不会停止连接、创建或清理库。
两者均创建不可覆盖 0600 证据；完整示例见[部署](../../ops/deploy/README.md#数据库恢复与保留)与[恢复手册](../../ops/deploy/RECOVERY.md)。

## 备份与历史接管

当前 schema 使用 `postgres-backup` / `postgres-backup-verify`，产生 custom-format archive 和配对 manifest；
外部 `pg_dump`/`pg_restore` 由部署环境提供。
备份核对前后迁移版本，verify 离线校验身份、SHA-256 与 archive 目录；详细步骤见[恢复手册](../../ops/deploy/RECOVERY.md)。

canonical v28 使用独立备份/verify/adoption：默认只读，执行需专用恢复点、`--execute`、逐字
`ADOPT LEGACY V28 <数据库名>` 和新 receipt。普通 backup/restore 不接受 v28 manifest。
bridge 或接管已提交后失败不能再次 adoption，须保持停机、复核普通前向迁移并收集异常证据；
精确格式、五语言规则和启动门槛见[v28 手册](../../ops/deploy/LEGACY_V28_ADOPTION.md)。

## 恢复与破坏性重建

`postgres-restore` 只向匹配 manifest 身份的同名空库恢复，要求零外部连接、可写主节点和无用户对象。
`--dry-run` 只读且不得提供 receipt；实际执行使用单事务 `pg_restore`，schema 后验通过后发布 receipt。
提交后校验或回执失败不等于数据库未变化，保持停机按[恢复手册](../../ops/deploy/RECOVERY.md)处理。
旧版恢复也核对完整 SQLx 历史前缀、每条成功状态及 checksum；缺行、未知版本或伪造 checksum 不能发布 clean 回执。

`postgres-rebuild` 默认只读；执行必须同时有精确目标、`--execute`、`REBUILD <数据库名>`、新 receipt，
以及 verified backup。只有明确可丢弃的一次性数据库可改用 `--allow-unrecoverable-ephemeral` 与
`EPHEMERAL <数据库名>`，禁止用于持久数据。
`--mode empty-for-restore` 强制备份且只准备空库；`fresh-schema` 在空库上执行当前迁移。
独立维护角色、零连接、catalog/TLS/advisory lock 门禁和 v2 回执 `true/false/null` 的处理均见恢复手册；
未知结果不能推断为未执行，也不能盲目重试 DROP。

## 安全约束

连接串、Cookie、密钥不进入参数、日志或回执。灾备文件使用受信私有目录、新路径与不可覆盖发布；
文件和父目录同步后才报告成功，mutation 前也同步恢复证据，不支持同步时失败关闭。
破坏性操作必须按独立手册精确确认目标和恢复点，不加入普通启动、迁移、unit 或开发脚本。

## 维护与验证

`cargo check --locked -p auctionctl` 只做默认 feature 静态编译；完整门禁见[开发指南](../../docs/development.md)。
静态检查不能证明备份可恢复或外部操作成功，运行证据见[验证状态](../../docs/status.md)。
