# auctionctl

配置、迁移和运维命令行工具。

源码布局：`src/main.rs` 只负责进程入口，`src/bootstrap/runtime.rs` 保留兼容装配边界，
`src/bootstrap/runtime/` 按 CLI 类型、分派、部署检查和 PostgreSQL 运维 artifact/命令拆分；其中
`runtime/postgres/commands/{mod,bootstrap,database_bootstrap,forum,rules,settlement,preflight,backup,delegation}.rs` 分别负责命令装配、
MoogleMail 凭据引导、缺失 PostgreSQL 目标库的非破坏性 bootstrap、论坛未知写入恢复、受控规则目录导入、历史结算账本只读核验、只读预检、备份/manifest 和专用
restore/legacy/rebuild owner 桥接；
`runtime/postgres/artifacts/` 又将外部 PostgreSQL 环境、manifest/hash、文件系统安全、外部进程、
来源预检和 stdin Cookie 分别放入 `environment.rs`、`manifest.rs`、`filesystem.rs`、`process.rs`、
`source.rs` 与 `input.rs`，`artifacts.rs` 只保留稳定导出，
`src/bootstrap/deployment_control/` 按总控装配、Unix 生命周期、systemd 适配、readiness 探针和 receipt
文件边界拆分，其中 `deployment_control/unix/` 又将生命周期、内部状态模型和路径/unit 校验分别放入
`lifecycle.rs`、`controller.rs`、`model.rs` 与 `validation.rs`，`unix/mod.rs` 只承载稳定包装，
`src/bootstrap/postgres_restore/` 按恢复编排、备份/模式证明、pg_restore 外部进程和不可覆盖 receipt 分别拆入
`postgres_restore/{mod,verification,process,receipt}.rs`；mod.rs 保留 execute、稳定验证出口和失败关闭顺序，
`src/bootstrap/postgres_legacy_v28/` 按 `backup.rs`（备份生成）、`verification.rs`（离线 archive/manifest
校验）、`adoption.rs`（接管编排）、`common.rs`（预检/后置证明）、`model.rs`（证据模型）和
`receipt.rs`（不可覆盖接管回执）拆分 canonical PostgreSQL v28 历史格式的专用边界，`mod.rs` 只保留稳定出口，
`src/bootstrap/postgres_rebuild/` 按 `model.rs`（授权/参数）、`execute.rs`（破坏性执行）、
`validation.rs`（确认和 URL 安全校验）、`postconditions.rs`（空库/迁移证明）以及
`receipt.rs`（不可覆盖 receipt 发布）拆分默认只读的破坏性重建边界；`mod.rs` 只保留稳定调用路径。
`src/bootstrap/runtime/mail_diagnostics/` 持有 MoogleMail 证据导出、只读 Sent 核对和历史展示投影回填的
PostgreSQL 完整档运维命令边界；这些命令不把敏感正文写入 stdout。

```bash
# 校验配置并显示解析结果（不回显凭据）
AUCTION_CONFIG_FILE=ops/config/examples/public.toml \
  cargo run -p auctionctl -- check-config public

# 独立数据库迁移；默认复用 systemd unit owner，并要求 Public/Admin/Worker 全部停止
cargo run -p auctionctl -- migrate

# PostgreSQL 初始建库；只在目标库缺失时 CREATE DATABASE，不运行迁移，也不修改已有库。
# 该命令与 migrate 使用相同的 systemd 状态目录锁和三角色停机门禁；执行成功后再显式运行 migrate。
AUCTION_CONFIG_FILE=/etc/auction/public.toml \
  cargo run -p auctionctl --features postgres -- \
    postgres-bootstrap \
    --confirm-database auction_production \
    --maintenance-secrets-file /etc/auction/postgres-maintenance.secret.toml

# 只读预检三进程配置；不会连接数据库、迁移 schema 或触发外部请求。
cargo run -p auctionctl --features postgres -- deployment-check --config-dir /etc/auction

# 生产切换前把敏感 TOML 的 group/other 权限作为硬门槛。
cargo run -p auctionctl --features postgres -- deployment-check --config-dir /etc/auction --strict-permissions

# 控制已安装的 systemd unit；start/restart 会严格预检、迁移并等待三角色 readiness。
sudo /opt/auction/bin/auctionctl deployment start --config-dir /etc/auction
sudo /opt/auction/bin/auctionctl deployment status
sudo /opt/auction/bin/auctionctl deployment restart --config-dir /etc/auction
sudo /opt/auction/bin/auctionctl deployment stop

# 一次性登记 MoogleMail 凭据；Cookie 只从标准输入读取，不放入参数或日志。
# 生产环境应使用仅运行账号可读的 secret 文件或 secret manager 管道：
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    bootstrap-mail-credentials --world main --label bot-primary \
    < /run/auction/secrets/mooglemail-main.cookie

# 离线解密并导出一条 MoogleMail 异常证据；不连接数据库、Worker 或远端服务。
# output 必须是新的私有路径，正文不会打印到 stdout。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    mail-evidence export 00000000-0000-4000-8000-000000000001 \
    --output /secure/auction-evidence/00000000-0000-4000-8000-000000000001.json

# 只读核对一条结算义务对应的 Sent；不会领取、对账、发送或修改状态。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    mail-evidence inspect-sent 00000000-0000-4000-8000-000000000001 \
    --output /secure/auction-evidence/sent-00000000-0000-4000-8000-000000000001.json

# 使用已认证的 parsed_message 诊断证据回填历史审查投影；可显式指定历史父密钥来源。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    mail-evidence backfill-display 00000000-0000-4000-8000-000000000001 \
    --key-source credential

# 把受控五语言 Markdown 目录首次导入 Admin 指向的 PostgreSQL；不会覆盖不同的已发布正文。
AUCTION_CONFIG_FILE=/etc/auction/admin.toml \
  cargo run -p auctionctl --features postgres -- \
    import-rules-directory \
    --directory /secure/auction-migration/rules \
    --command-id 123e4567-e89b-42d3-a456-426614174000 \
    --actor legacy-rules-import

# 只读列出论坛写入结果未知、只读对账中或等待人工授权的任务。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    forum-write-recovery-list --limit 100

# 精确标题对账未命中后，显式授权某一拍卖再次尝试创建主题。
# command-id 必须稳定且非零；confirm-write 必须逐字包含同一 auction UUID。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    authorize-forum-write-retry \
    --auction-id 00000000-0000-4000-8000-000000000001 \
    --command-id 00000000-0000-4000-8000-000000000002 \
    --actor auction-operator \
    --confirm-write 'AUTHORIZE FORUM WRITE 00000000-0000-4000-8000-000000000001'

# 在同一 Serializable 只读快照中核验已迁移的旧 settlement_complete/OK 账本。
# 该命令只输出脱敏身份与数量，不补 fence、不重新结算，也不追加任何数据库事实。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    verify-legacy-settlement \
    --auction-id 00000000-0000-4000-8000-000000000001

# 只读收集 PostgreSQL 保留盘点；目标证据文件不可覆盖，且不会归档或删除数据。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    retention-inventory \
    --confirm-database auction_production \
    --output /secure/auction-evidence/retention-2026-08-23.json

# 只读检查 PostgreSQL 恢复目标；不会执行 pg_restore、创建或删除数据库。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    recovery-preflight \
    --confirm-database auction_restore \
    --output /secure/auction-evidence/recovery-preflight-2026-08-23.json

# 创建 PostgreSQL custom-format 备份和配对 manifest；archive/manifest 均不可覆盖。
# 运行账号需要能够调用 pg_dump 和 pg_restore。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    postgres-backup \
    --confirm-database auction_production \
    --output /secure/auction-backups/auction-2026-08-23.dump

# 在不连接数据库的情况下校验 archive、SHA-256、manifest 配对关系和 pg_restore 目录。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    postgres-backup-verify \
    --confirm-database auction_production \
    --manifest /secure/auction-backups/auction-2026-08-23.dump.manifest

# 为停止服务后的 canonical PostgreSQL v28 历史格式创建专用恢复点。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    postgres-legacy-v28-backup \
    --confirm-database auction_production \
    --output /secure/auction-backups/auction-legacy-v28-2026-08-23.dump

# 离线复核 v28 专用 archive/manifest；普通 postgres-backup-verify 不接受这种 manifest。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    postgres-legacy-v28-backup-verify \
    --confirm-database auction_production \
    --manifest /secure/auction-backups/auction-legacy-v28-2026-08-23.dump.manifest

# 接管命令默认只读；可先不提供备份查看来源库是否精确满足 v28 前置条件。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    postgres-adopt-legacy-v28 \
    --confirm-database auction_production

# 真正接管还要求已验证备份、逐字确认和新的不可覆盖 receipt。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    postgres-adopt-legacy-v28 \
    --confirm-database auction_production \
    --backup-manifest /secure/auction-backups/auction-legacy-v28-2026-08-23.dump.manifest \
    --execute \
    --confirm-adoption 'ADOPT LEGACY V28 auction_production' \
    --receipt /secure/auction-evidence/adopt-v28-2026-08-23.json

# 停止全部角色并准备同名空库后，先只读验证实际恢复的全部前置条件。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    postgres-restore \
    --confirm-database auction_production \
    --manifest /secure/auction-backups/auction-2026-08-23.dump.manifest \
    --dry-run

# 单事务恢复并发布不可覆盖的成功 receipt；命令不会 DROP/CREATE 数据库或停止连接。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    postgres-restore \
    --confirm-database auction_production \
    --manifest /secure/auction-backups/auction-2026-08-23.dump.manifest \
    --receipt /secure/auction-evidence/restore-2026-08-23.json

# 重建命令默认只读：验证当前 Auction 源库、零外部连接、备份证据、维护权限和 catalog 状态。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    postgres-rebuild \
    --confirm-database auction_production \
    --mode empty-for-restore \
    --backup-manifest /secure/auction-backups/auction-2026-08-23.dump.manifest

# 实际 DROP/CREATE 还必须显式 execute、逐字确认目标，并预留新的私有 receipt。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    postgres-rebuild \
    --confirm-database auction_production \
    --mode empty-for-restore \
    --backup-manifest /secure/auction-backups/auction-2026-08-23.dump.manifest \
    --maintenance-secrets-file /etc/auction/postgres-rebuild.secret.toml \
    --execute \
    --confirm-rebuild 'REBUILD auction_production' \
    --receipt /secure/auction-evidence/rebuild-2026-08-23.json
```

`check-config` 接受 `public` / `admin` / `worker`，输出 JSON 格式的有效配置安全投影：监听模式与
非敏感路径、数据库后端类型、缓存 TTL、tracing/metrics 开关、HTTP 安全计数、Worker control
边界、Admin 装备来源请求间隔、有效 MoogleMail 启用状态/world、allowlist 总数、装备过滤开关、
模板数量和配置来源状态。
它不会输出数据库 URL、Redis URL、Cookie、会话签名密钥、OTLP headers 或
任何 `.keyenv` 内容。

`mail-evidence export <UUID> --output <new-path>` 只在本地加载 Worker 配置和诊断目录，按 UUID
解密异常证据并使用 `create_new`/`0600` 创建 JSON 输出；可用 `--key-source credential|equipment`
指定历史父密钥来源，省略时优先 credential，缺失时使用 equipment。命令不连接数据库、Worker、MoogleMail
或其他远端服务，也不把正文输出到 stdout。历史导出必须保留对应旧父密钥备份；该命令不会自动轮换密钥。

`mail-evidence inspect-sent <obligation-id> --output <path>` 只属于 PostgreSQL 完整档的离线 Sent
诊断，且仅限 `withdrawn_return` 单件装备、零 Credits、`COD=0` 的义务：按页数上限和总 deadline 读取 Sent，
要求唯一且精确匹配 world、recipient、canonical subject 和装备 ID，并标记 `time_unverified`。它不是通用
payment 核对或已送达证明。即使查询得到 `collected`，也只表示外部事实尚未证明，不会自动 reconcile 或 send；
输出使用新的私有文件路径，命令不修改 delivery、fulfillment、lease 或远端邮件。

`mail-evidence backfill-display <evidence-id> [--key-source credential|equipment]` 只接受认证的
`parsed_message` 证据，并以 world/mailbox/mid/content digest 精确匹配、CAS 和追加审计回填 `sender`、`subject`
及脱敏展示快照。相同幂等身份但内容不同必须冲突；找不到或不匹配证据时保持“未留存”。命令不能把诊断证据
当作业务事实或幂等输入。

`auctionctl` 的退出码是稳定的：`0` 表示成功，`2` 表示 CLI 用法或配置错误，`3` 表示数据库/外部
依赖或前置条件失败，`4` 表示 systemd 部署编排失败，`5` 表示其它运维操作失败。错误正文沿用
配置解析器的脱敏策略；调用方应依赖退出码判断结果，不要解析错误文本。

`deployment-check` 读取 `public.toml`、`admin.toml` 和 `worker.toml`，检查三者引用同一份普通、
非符号链接的公共配置，并检查敏感文件、runtime 文件、数据库档位与实际目标身份、Public/Admin TCP/Unix 监听冲突、Admin/Worker control socket 一致性，
以及装备、用户登录和外部凭据三组数据密钥的一致性。启用中的 Worker control 还必须具备安全
runtime 目录、group 与显式 UID/GID allowlist 配置。
数据库目标检查由 `src/bootstrap/runtime/deployment_identity/` 拥有部署限制，并复用 storage 的 SQLx
连接参数解析；允许不同数据库账号访问同一个目标。PostgreSQL 必须显式指定 host、port 和数据库，
支持 SQLx 的 host/hostaddr/port/dbname 查询参数覆盖以及 Unix socket；不能依赖环境默认值、使用
未知参数或 `options` 会话覆盖。DNS 别名不做网络解析，部署配置应使用同一规范主机名。
标准 IPv6 URI 的方括号只用于 URI 表达；SQLx 实际 TCP 连接会使用去括号的地址，外部备份/恢复
工具的 `PGHOST` 和临时 pgpass 同步使用裸 IPv6（pgpass 的冒号仍按原规则转义）。查询参数覆盖、
Unix socket 和显式目标/TLS 门禁保持不变，不将连接原文或凭据写入诊断。
SQLite 必须使用绝对磁盘路径，已有文件按实际路径比较，新文件按已存在的实际父目录比较；
不允许相对路径、内存库或自定义 VFS，避免 CLI 与 systemd 工作目录不同造成同名异库。
接管 Admin runtime 的任一 `operations` 组时，Admin 与 Worker 必须引用同一份 runtime 普通文件；
预检按规范文件路径核对身份，并始终核对两者最终经济模板、结算模板与周排期一致，防止管理页已保存
但 Worker 未接入同一来源。该检查同样不读取外部服务或改写任务状态。
默认只对敏感文件权限过宽给出警告；生产部署应使用 `--strict-permissions`。该预检不会连接
数据库、执行迁移、启动服务或访问 MoogleMail/E-Hentai。

`deployment start/restart/stop/status` 是 Linux/systemd 宿主机总控。单角色 PID、信号、停止超时、
自动重启和日志由 systemd 拥有；总控校验 unit 已加载且其 `AUCTION_CONFIG_FILE` 与指定绝对
`config-dir` 精确一致。`start` 在三个 unit 全部停止时执行严格预检、监听地址占用检查和
forward-only migration；只有 `inactive`/`failed` 且 `MainPID=0` 才视为停止，
`deactivating` 等转换状态必须先收敛。随后按 Worker → Admin → Public 启动并分别读取 Worker Unix control
`/v1/health/ready`、Public 实际选择的 TCP 或 Unix `/health/ready`，以及 Admin runtime 实际选择的 TCP 或 Unix
`/health/ready`。任一角色失败时只按反序停止本次启动的 unit，
不会回退已经提交的 migration。`restart` 会先完成配置/unit 预检，再按 Public → Admin → Worker
停止并重新执行同一流程；`stop` 的终止等待和最终强制策略由各 unit 的 `TimeoutStopSec` 管理。

配置绑定仅支持 `Environment` 的简单赋值（可整体用双引号包裹，不能含空白或转义）。
`EnvironmentFiles`、`UnsetEnvironment`、`PassEnvironment` 和 `PAMName` 非空时预检失败，
避免其他环境来源覆盖或删除 `AUCTION_CONFIG_FILE`；失败信息不输出环境值。TCP/Unix readiness
均使用异步非阻塞连接，连接、请求写入和状态行读取共用最多 1 秒预算，且不超过当前角色的剩余启动时间。

总控要求 Worker 启用 `[worker.control]`，要求私有 `--state-dir` 已存在、路径规范化、归当前
有效用户所有且权限至多 `0700`，并以非阻塞文件锁拒绝并发控制操作。配置目录和三个主 TOML
不得是符号链接或允许 group/other 写入。成功后原子发布 `startup-order.v1.json`；回执只包含
unit、systemd `InvocationID`、MainPID、单调启动时间和观察时间，不包含 TOML 正文、连接串或凭据。
如果三个 unit 已经运行，`start` 会验证 readiness 与启动时间顺序后以 `adopted` 模式刷新回执。
推荐通过 [`ops/deploy/systemd/auction-stack.service.example`](../../ops/deploy/systemd/auction-stack.service.example)
在开机时进入同一控制边界，而不要分别 enable 三个角色 unit。

独立 `migrate` 同样读取 `SystemdControllerArgs` 的默认 unit（`auction-worker.service`、
`auction-admin-api.service`、`auction-public-api.service`），先持有同一部署控制锁，再读取三个 unit
状态；任一角色处于运行或状态转换中、unit 状态读取失败或部署锁被占用时都会失败关闭，数据库迁移不会
开始。需要由总控在受控启动/重启流程中迁移时，仍由 `deployment start/restart` 内部调用
`migrate_config`，不经过这个独立命令 guard；这是部署启动的受控迁移路径。

`postgres-bootstrap` 是普通迁移之外的显式 PostgreSQL 初始建库命令。它只读取 `Public` 配置的
`database_url`，要求 `--confirm-database` 与 URL path 完全一致，并可选读取仅运行账号可读的
maintenance sensitive TOML；应用与维护 URL 都必须显式给出 host、非零 port 和数据库名，
不能依赖 `PGHOST`/`PGPORT` 补齐目标；未提供维护文件时只把应用 URL 的数据库名切换到 `postgres`
维护库。端点按实际 SQLx 解析结果比较，并沿用下述 rebuild 的严格查询参数和 TLS 约束。命令先取得与 `migrate` 相同的部署状态目录锁，再确认 Public/Admin/Worker 均处于明确的
`inactive`/`failed` 或 `not-found` 收敛状态；锁生命周期覆盖 PostgreSQL catalog 检查、建库和应用
连接后置验证。PostgreSQL 适配器随后取得与 rebuild 共用的集群级 advisory lock，确认 writable
primary、维护角色 `CREATEDB`、应用角色存在且可分配 owner，拒绝 `postgres`/`template0`/`template1`
及不安全数据库名。

目标库缺失时只执行安全 identifier quoting 的 `CREATE DATABASE <target> OWNER <application role>`，
然后验证 catalog owner 和应用 URL 的 `current_database()`/`current_user`；目标库已存在时绝不
修改它，只有 owner 一致且应用 URL 可连接才以 `already_exists` 幂等成功。连接异常、权限不足、
名称非法、目标 owner 不一致或身份后置验证失败均确定失败关闭。该命令绝不运行迁移；标准两步顺序
是先执行 `postgres-bootstrap`，再在同一停机门禁下显式执行普通 `migrate`。普通 `migrate` 不具备
`CREATEDB` 或隐式建库权限。

## Cargo feature

`migrate` 和 `deployment start/restart` 的 migration 阶段需要对应后端的驱动：

- `postgres` → `auction-storage/postgres`
- `sqlite` → `auction-storage/sqlite`
- `cache-redis` → `auction-cache/redis`；只在有效 Admin 配置选择 Redis 时供规则导入失效缓存

默认不带驱动，因此默认构建下 `check-config` 只能校验 URL scheme 合法性，`migrate` 和部署启动
会在启动任何 unit 前明确提示需要启用 `postgres` 或 `sqlite` feature。启用对应 feature 后，工具会按连接串选择并执行
对应迁移树；SQLite 只执行已声明的 Lite/共有能力，不会执行 PostgreSQL 专属对象。
迁移前会先检查目标是否为空或已有 SQLx 迁移历史；发现没有 Auction 迁移标记但存在其他用户对象时会
失败关闭，不会把未知数据库当作空库初始化。

`bootstrap-mail-credentials` 只允许 `main` / `isekai`，要求对应 world 已在 Worker 配置中启用，
并使用敏感 TOML 中的 `credential_data_key` 加密后写入 PostgreSQL。Cookie 通过标准输入读取并在
导入服务完成后释放；命令不会回显 Cookie、密文或数据密钥。

`import-rules-directory` 只支持 PostgreSQL 完整档，并加载 Admin 主配置以取得数据库和可选缓存边界。
来源目录必须同时包含普通、非符号链接的 `en.md`、`zh-CN.md`、`zh-TW.md`、`ja.md` 和 `ko.md`；
命令按本仓库导入契约移除单个开头 BOM、把首个非空行作为标题，并在写入前完成五文件校验及
五语言数据库预检。数据库已有相同标题和正文时报告 `preserved`，任一语言不同则在首次写入前失败，
缺失语言才通过既有 Admin 规则 CAS、幂等账本和追加审计发布。各语言命令 UUID 由批次
`--command-id` 稳定派生；中途依赖失败后应以同一批次 UUID 重试。JSON 结果包含每种语言的动作、
revision、正文 SHA-256 和实际写入命令 UUID。缓存失效失败不会回滚已提交正文，但会列入
`cache_invalidation_failures`；配置选择 Redis 时必须同时启用 `postgres,cache-redis`，否则命令失败
关闭而不会静默禁用缓存。

`forum-write-recovery-list` 只读返回 PostgreSQL 中需要论坛写入对账的安全投影，不发起远端请求或
修改状态。Worker 会先按精确标题 GET；命中时只补本地发布证明，未命中时停在
`write_authorization_required`。此后只有 `authorize-forum-write-retry` 可以写入幂等授权命令、
追加审计并打开下一次 POST；它要求非零 auction/command UUID、规范 actor，以及逐字等于
`AUTHORIZE FORUM WRITE <auction UUID>` 的二次确认。同一 command ID 搭配不同摘要会失败冲突，
两条命令均只支持 PostgreSQL 完整档，不能用它们绕过精确标题对账或盲目重复写入。

`verify-legacy-settlement` 只支持 PostgreSQL 完整档，并要求目标已经是带完整 workflow、claim、
complete 身份和 `settlement_complete/OK` 账本的 settled 拍卖。它在
`SERIALIZABLE READ ONLY DEFERRABLE` 快照内同时读取命令账本、结算摘要、lot 金额和义务种类数量，
逐字复现历史 complete 请求摘要并复用应用层严格重建校验。缺失账本、当前格式账本、身份漂移、结果
损坏或权威事实不一致都会失败关闭；成功 JSON 只包含稳定 UUID、due 时间、状态版本和脱敏计数。
该入口没有通用 `settle` 回退，不清空 completion fence，也不写命令、审计、实时事件或任何结算事实。

`retention-inventory` 只支持 PostgreSQL 完整档，要求 `--confirm-database` 与连接实际返回的
`current_database()` 完全一致。命令会把版本化的
[`retention-policy.v1.json`](../../ops/deploy/retention-policy.v1.json) 编译进制品，并在
`REPEATABLE READ READ ONLY` 事务中读取 Auction schema 身份、SQLx 迁移状态、public 关系大小、
`pg_stat_user_tables` 行数估计、政策声明表的缺失情况、realtime 诊断水位和 public 外键依赖；
结果以不可覆盖的 0600 JSON 文件保存。证据包含政策 SHA-256、数据族汇总和待人工决定项。
realtime consumer 最小 offset 只作为诊断水位，外键依赖在政策明确前视为阻断因素。
`eligibility_assessed=false` 与
`eligibility_authorized=false` 固定不变，不能作为归档或删除授权；备份、恢复和任何 mutation
仍需独立的生产契约。

`recovery-preflight` 同样只支持 PostgreSQL 完整档，要求精确确认 `current_database()`。它在只读
事务中检查主节点可写状态、其他连接、数据库大小、用户对象数量、Auction schema identity 和 SQLx
迁移历史，并将 `restore_preflight_passed` 写入不可覆盖证据。通过只表示目标满足空库/无其他连接/
可写主节点的基础条件，不验证 backup archive 或恢复内容，也不会停止连接、执行 `pg_restore` 或
覆盖现有对象。user mapping 只经普通数据库 owner 可读的 `pg_catalog.pg_user_mappings` 安全视图
计数，不要求为该只读盘点授予 superuser。

`postgres-backup` 只支持 PostgreSQL 完整档，要求源库通过 schema identity、SQLx 迁移完整性和数据库
身份检查；命令调用外部 `pg_dump --format=custom`，随后用 `pg_restore --list` 校验 archive，并在
前后核对迁移版本。密码不会出现在参数或日志中，而是短生命周期写入仅运行账号可读的临时
`.pgpass`；archive 和 manifest 使用 0600、不可覆盖的私有文件。`postgres-backup-verify` 只读取
本地配对文件并重新计算 SHA-256/检查 archive 目录，不连接数据库，也不执行恢复。
调用外部 PostgreSQL 工具的连接 profile 要求敏感 `database_url` 显式提供 host 和 port
（包括默认端口 `5432`）；缺失时直接拒绝。工具清除继承的 `PGHOST`、`PGHOSTADDR`、`PGPORT`
后只从同一 URL 设置目标，避免 SQLx 预检和 libpq 工具使用不同的机器环境默认值；
普通备份、恢复和 v28 备份/接管的同一连接 profile 都遵守此限制。

`postgres-adopt-legacy-v28` 是普通 `migrate` 之外的显式 v28 历史格式接管边界。它只接受 PostgreSQL 18.x、
唯一且干净的 `schema_migrations version=28`、本项目定义的精确 canonical public 表集合、有效
schema identity、零其他 session、尚不存在 SQLx 历史，并证明 policy、lot 领先投影和每条 bid 的
realtime 截止证据都可无损转换。默认只读；实际执行必须先用
`postgres-legacy-v28-backup`/`postgres-legacy-v28-backup-verify` 建立专用恢复点，再提供逐字确认和
新的私有 receipt。实际执行取得 migration advisory lock 后，会在同一物理连接中重新完成完整只读
预检，再由 bridge 在 Serializable 事务和独占表锁内补齐 Rust v5 兼容结构，将等价覆盖的
SQLx `1..=5` 与 `7` 写为带当前 checksum 的基线，再运行其余前向迁移。普通备份/恢复命令故意不
接受 v28 专用 manifest，避免把来源 migration v28 误报为当前 SQLx schema。完整停机、接管、失败
恢复和证据保管顺序见
[`ops/deploy/LEGACY_V28_ADOPTION.md`](../../ops/deploy/LEGACY_V28_ADOPTION.md)。

如果 bridge 已提交而后续 SQLx migration 失败，或接管已提交但 postflight/receipt 发布失败，全部
角色必须继续停止；此时不得再次运行接管命令。先运行普通 `auctionctl migrate` 复核前向迁移，再
保存 recovery preflight、数据库审计和新的当前 schema 备份作为异常证据。只有正常执行中的 schema
identity 和全部内嵌迁移都通过后才会发布 `auction-postgres-legacy-v28-adoption.v1` receipt。

`postgres-restore` 复用同一 manifest/哈希校验，并要求 `--confirm-database` 同时匹配 Worker
连接和 manifest 数据库身份。目标必须是可写主节点、没有其他连接且不存在任何用户 schema、类型、
关系、函数、扩展或其他用户对象；工具自身的预检连接会在 `pg_restore` 前显式关闭。mutation 前会
再次校验 archive/manifest，随后只执行
`pg_restore --exit-on-error --single-transaction --no-owner --no-privileges`。成功后必须读取有效
Auction schema identity 和 manifest 记录的 clean SQLx migration 版本，才会发布不可覆盖的 0600
JSON receipt。archive 版本可以旧于当前二进制，恢复后通过正常 `auctionctl migrate` 前向升级；
高于当前内嵌迁移版本的 archive 会在 mutation 前被拒绝。完整操作顺序见
[`ops/deploy/RECOVERY.md`](../../ops/deploy/RECOVERY.md)。停止外部进程和 retention mutation
仍由部署方拥有；restore 本身继续不创建、删除或清理数据库。

`postgres-rebuild` 是独立的 PostgreSQL 18 完整档破坏性契约，不会由 `migrate`、deployment
总控或 systemd unit 隐式触发。未提供 `--execute` 时只做读取和本地 archive 校验；实际执行必须
同时提供精确 `--confirm-database`、逐字 `REBUILD <数据库名>`、新的私有 receipt，以及以下二者之一：

- 已通过 archive、SHA-256、数据库身份和 schema 版本检查的 `--backup-manifest`；
- 只允许 disposable 数据库使用的 `--allow-unrecoverable-ephemeral`，并逐字确认
  `EPHEMERAL <数据库名>`。

目标必须已经是当前且 clean 的 Auction schema，Public/Admin/Worker 和其他客户端必须先停止，
维护连接必须观察到零目标 session、零 prepared transaction、零 subscription 和零活动 logical slot。
生产环境推荐通过 `--maintenance-secrets-file` 使用独立 owner-only TOML；其中唯一的
`database_url` 必须指向同 endpoint/TLS profile 的 `postgres` 维护库，且不会进入命令行、日志或
receipt。连接源库之前，两条 URL 都必须显式给出单一 host、非零 port 和数据库名；默认端口也要
写明，避免 `PGPORT` 让源库预检和破坏性操作落到不同实例。通过实际 SQLx 解析结果比较端点，
查询参数只接受 `application_name` 和 `sslmode`；SQLx 会忽略的 `ssl=true`、`connect_timeout`
在此拒绝。两边 `sslmode` 必须相同或同时省略，不能把继承环境的省略值当成显式 `prefer`。
未提供该文件时仅从 Worker URL 替换数据库名为 `postgres`，其余目标与策略不变。对应数据库角色必须是 superuser，
或同时是目标 owner 并拥有 `CREATEDB`。为避免静默降低安全策略，存在自定义
database ACL 或 database/role setting 时失败关闭。重建以 `template0` 保留原 owner、UTF8 编码、
locale provider、tablespace 和 connection limit，并在集群 advisory lock 下用
`DROP DATABASE ... WITH (FORCE)` 捕获预检后的连接竞态。
自定义 tablespace 仍要求显式 `CREATE` 权限；目标为 `pg_default` 时省略显式 TABLESPACE 子句，
以符合普通 `CREATEDB` owner 的 PostgreSQL 权限语义。

`--mode empty-for-restore` 只创建并证明空库，且强制要求 backup manifest；随后仍需显式运行
`postgres-restore`。`--mode fresh-schema` 在空库证明后运行当前内嵌迁移，允许 verified backup 或
明确的一次性环境豁免。破坏性步骤开始后若响应丢失或后置验证失败，命令会尽可能发布
`auction-postgres-rebuild.v2` 回执：`database_dropped`、`database_recreated` 和 `forced_drop`
使用 `true`/`false`/`null`，分别表示已证明发生、已证明未发生、结果未知；DROP/CREATE 执行错误
在缺少独立证明时记录 `mutation_unknown`，不会把 I/O 错误推断成 `false`。已证明完成的后置失败
仍为 `postcondition_failed`，并保留 `mutation_failed` 的明确失败语义。历史 `auction-postgres-rebuild.v1` 回执不改写，读取方
必须按 `format` 区分，不能把 v2 的 `null` 当作 `false`。失败回执写入错误仅附加到原操作错误链，
不会掩盖原始失败。出现上述结果时必须保持全部角色停止，先核对 catalog/OID 与备份再人工恢复，
不能把进程退出失败误当成数据库未变化，也不能盲目再次执行 DROP。

## 安全约束

破坏性数据库操作必须使用独立命令、明确运行手册、精确目标确认、可验证备份和不可覆盖回执。
不得把 `DROP DATABASE`、恢复或 retention mutation 放入启动、迁移、unit 或普通开发脚本。

灾备 archive、manifest 与回执发布只有在文件内容和父目录同步完成后才报告成功；恢复、重建及
v28 接管在 mutation 前也同步既有恢复证据。跨目录发布会同步源、目标两个目录；不支持目录同步的
平台或文件系统失败关闭。文件可读或 SHA-256 匹配不代替这项持久化证明。
