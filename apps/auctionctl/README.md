# auctionctl

配置、迁移和运维命令行工具。

源码布局：`src/main.rs` 只负责进程入口，`src/bootstrap/runtime.rs` 保留兼容装配边界，
`src/bootstrap/runtime/` 按 CLI 类型、分派、部署检查和 PostgreSQL 运维 artifact/命令拆分；其中
`runtime/postgres/artifacts/` 又将外部 PostgreSQL 环境、manifest/hash、文件系统安全、外部进程、
来源预检和 stdin Cookie 分别放入 `environment.rs`、`manifest.rs`、`filesystem.rs`、`process.rs`、
`source.rs` 与 `input.rs`，`artifacts.rs` 只保留稳定导出，
`src/bootstrap/deployment_control/` 按总控装配、Unix 生命周期、systemd 适配、readiness 探针和 receipt
文件边界拆分，其中 `deployment_control/unix/` 又将生命周期、内部状态模型和路径/unit 校验分别放入
`lifecycle.rs`、`model.rs` 与 `validation.rs`，`unix/mod.rs` 只承载稳定包装，
`src/bootstrap/postgres_restore.rs` 独立持有 PostgreSQL 恢复边界，
`src/bootstrap/postgres_legacy_v28/` 按 `backup.rs`（备份生成）、`verification.rs`（离线 archive/manifest
校验）、`adoption.rs`（接管编排）、`common.rs`（预检/后置证明）、`model.rs`（证据模型）和
`receipt.rs`（不可覆盖接管回执）拆分 Auction-BF v28 的专用边界，`mod.rs` 只保留稳定出口，
`src/bootstrap/postgres_rebuild/` 按 `model.rs`（授权/参数）、`execute.rs`（破坏性执行）、
`validation.rs`（确认和 URL 安全校验）、`postconditions.rs`（空库/迁移证明）以及
`receipt.rs`（不可覆盖 receipt 发布）拆分默认只读的破坏性重建边界；`mod.rs` 只保留稳定调用路径。

```bash
# 校验配置并显示解析结果（不回显凭据）
AUCTION_CONFIG_FILE=ops/config/examples/public.toml \
  cargo run -p auctionctl -- check-config public

# 数据库迁移（按 database_url 选择 PostgreSQL 完整迁移或 SQLite Lite 子集）
cargo run -p auctionctl -- migrate

# 只读预检三进程配置；不会连接数据库、迁移 schema 或触发外部请求。
cargo run -p auctionctl -- deployment-check --config-dir /etc/auction

# 生产切换前把敏感 TOML 的 group/other 权限作为硬门槛。
cargo run -p auctionctl -- deployment-check --config-dir /etc/auction --strict-permissions

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

# 为停止服务后的 canonical Auction-BF PostgreSQL v28 创建专用恢复点。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    postgres-legacy-v28-backup \
    --confirm-database auction_production \
    --output /secure/auction-backups/auction-bf-v28-2026-08-23.dump

# 离线复核旧版专用 archive/manifest；普通 postgres-backup-verify 不接受这种 manifest。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    postgres-legacy-v28-backup-verify \
    --confirm-database auction_production \
    --manifest /secure/auction-backups/auction-bf-v28-2026-08-23.dump.manifest

# 接管命令默认只读；可先不提供备份查看旧库是否精确满足 v28 前置条件。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    postgres-adopt-legacy-v28 \
    --confirm-database auction_production

# 真正接管还要求已验证备份、逐字确认和新的不可覆盖 receipt。
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  cargo run -p auctionctl --features postgres -- \
    postgres-adopt-legacy-v28 \
    --confirm-database auction_production \
    --backup-manifest /secure/auction-backups/auction-bf-v28-2026-08-23.dump.manifest \
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
边界和配置来源状态。它不会输出数据库 URL、Redis URL、Cookie、会话签名密钥、OTLP headers 或
任何 `.keyenv` 内容。

`auctionctl` 的退出码是稳定的：`0` 表示成功，`2` 表示 CLI 用法或配置错误，`3` 表示数据库/外部
依赖或前置条件失败，`4` 表示 systemd 部署编排失败，`5` 表示其它运维操作失败。错误正文沿用
配置解析器的脱敏策略；调用方应依赖退出码判断结果，不要解析错误文本。

`deployment-check` 读取 `public.toml`、`admin.toml` 和 `worker.toml`，检查三者的敏感文件、
runtime 文件、数据库档位、Public/Admin TCP/Unix 监听冲突、Admin/Worker control socket 一致性，
以及装备、用户登录和外部凭据三组数据密钥的一致性。启用中的 Worker control 还必须具备安全
runtime 目录、group 与显式 UID/GID allowlist 配置。
默认只对敏感文件权限过宽给出警告；生产部署应使用 `--strict-permissions`。该预检不会连接
数据库、执行迁移、启动服务或访问 MoogleMail/E-Hentai。

`deployment start/restart/stop/status` 是 Linux/systemd 宿主机总控。单角色 PID、信号、停止超时、
自动重启和日志由 systemd 拥有；总控校验 unit 已加载且其 `AUCTION_CONFIG_FILE` 与指定绝对
`config-dir` 精确一致。`start` 在三个 unit 全部停止时执行严格预检、监听地址占用检查和
forward-only migration，再按 Worker → Admin → Public 启动并分别读取 Worker Unix control
`/v1/health/ready`、Public 实际选择的 TCP 或 Unix `/health/ready`，以及 Admin runtime 实际选择的 TCP 或 Unix
`/health/ready`。任一角色失败时只按反序停止本次启动的 unit，
不会回退已经提交的 migration。`restart` 会先完成配置/unit 预检，再按 Public → Admin → Worker
停止并重新执行同一流程；`stop` 的终止等待和最终强制策略由各 unit 的 `TimeoutStopSec` 管理。

总控要求 Worker 启用 `[worker.control]`，要求私有 `--state-dir` 已存在、路径规范化、归当前
有效用户所有且权限至多 `0700`，并以非阻塞文件锁拒绝并发控制操作。配置目录和三个主 TOML
不得是符号链接或允许 group/other 写入。成功后原子发布 `startup-order.v1.json`；回执只包含
unit、systemd `InvocationID`、MainPID、单调启动时间和观察时间，不包含 TOML 正文、连接串或凭据。
如果三个 unit 已经运行，`start` 会验证 readiness 与启动时间顺序后以 `adopted` 模式刷新回执。
推荐通过 [`ops/deploy/systemd/auction-stack.service.example`](../../ops/deploy/systemd/auction-stack.service.example)
在开机时进入同一控制边界，而不要分别 enable 三个角色 unit。

## Cargo feature

`migrate` 和 `deployment start/restart` 的 migration 阶段需要对应后端的驱动：

- `postgres` → `auction-storage/postgres`
- `sqlite` → `auction-storage/sqlite`

默认不带驱动，因此默认构建下 `check-config` 只能校验 URL scheme 合法性，`migrate` 和部署启动
会在启动任何 unit 前明确提示需要启用 `postgres` 或 `sqlite` feature。启用对应 feature 后，工具会按连接串选择并执行
对应迁移树；SQLite 只执行已声明的 Lite/共有能力，不会执行 PostgreSQL 专属对象。
迁移前会先检查目标是否为空或已有 SQLx 迁移历史；发现没有 Auction 迁移标记但存在其他用户对象时会
失败关闭，不会把未知数据库当作空库初始化。

`bootstrap-mail-credentials` 只允许 `main` / `isekai`，要求对应 world 已在 Worker 配置中启用，
并使用敏感 TOML 中的 `credential_data_key` 加密后写入 PostgreSQL。Cookie 通过标准输入读取并在
导入服务完成后释放；命令不会回显 Cookie、密文或数据密钥。

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
覆盖现有对象。

`postgres-backup` 只支持 PostgreSQL 完整档，要求源库通过 schema identity、SQLx 迁移完整性和数据库
身份检查；命令调用外部 `pg_dump --format=custom`，随后用 `pg_restore --list` 校验 archive，并在
前后核对迁移版本。密码不会出现在参数或日志中，而是短生命周期写入仅运行账号可读的临时
`.pgpass`；archive 和 manifest 使用 0600、不可覆盖的私有文件。`postgres-backup-verify` 只读取
本地配对文件并重新计算 SHA-256/检查 archive 目录，不连接数据库，也不执行恢复。

`postgres-adopt-legacy-v28` 是普通 `migrate` 之外的显式旧库接管边界。它只接受 PostgreSQL 18.x、
唯一且干净的 `schema_migrations version=28`、精确的 canonical Auction-BF public 表集合、有效
schema identity、零其他 session、尚不存在 SQLx 历史，并证明 policy、lot 领先投影和每条 bid 的
realtime 截止证据都可无损转换。默认只读；实际执行必须先用
`postgres-legacy-v28-backup`/`postgres-legacy-v28-backup-verify` 建立专用恢复点，再提供逐字确认和
新的私有 receipt。bridge 在 Serializable 事务和独占表锁内补齐 Rust v5 兼容结构，将等价覆盖的
SQLx `1..=5` 与 `7` 写为带当前 checksum 的基线，再运行其余前向迁移。普通备份/恢复命令故意不
接受旧版专用 manifest，避免把 golang-migrate v28 误报为当前 Rust schema。完整停机、接管、失败
恢复和证据保管顺序见
[`ops/deploy/LEGACY_V28_ADOPTION.md`](../../ops/deploy/LEGACY_V28_ADOPTION.md)。

如果 bridge 已提交而后续 SQLx migration 失败，全部角色必须继续停止，只运行普通
`auctionctl migrate` 继续前向迁移；此时不得再次运行接管命令。只有 schema identity 和全部内嵌
迁移都通过后才会发布 `auction-postgres-legacy-v28-adoption.v1` receipt。

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
receipt。未提供该文件时才从 Worker `database_url` 派生维护连接。对应数据库角色必须是 superuser，
或同时是目标 owner 并拥有 `CREATEDB`。为避免静默降低安全策略，存在自定义
database ACL 或 database/role setting 时失败关闭。重建以 `template0` 保留原 owner、UTF8 编码、
locale provider、tablespace 和 connection limit，并在集群 advisory lock 下用
`DROP DATABASE ... WITH (FORCE)` 捕获预检后的连接竞态。

`--mode empty-for-restore` 只创建并证明空库，且强制要求 backup manifest；随后仍需显式运行
`postgres-restore`。`--mode fresh-schema` 在空库证明后运行当前内嵌迁移，允许 verified backup 或
明确的一次性环境豁免。破坏性步骤开始后若建库或后置验证失败，命令会尽可能发布状态为
`mutation_failed`/`postcondition_failed` 的 `auction-postgres-rebuild.v1` 回执；出现这类结果时必须
保持全部角色停止并按回执状态人工恢复，不能把进程退出失败误当成数据库未变化。

## 安全约束

破坏性数据库操作必须使用独立命令、明确运行手册、精确目标确认、可验证备份和不可覆盖回执。
不得把 `DROP DATABASE`、恢复或 retention mutation 放入启动、迁移、unit 或普通开发脚本。
