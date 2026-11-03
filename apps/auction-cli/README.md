# auction-cli

[文档导航](../../docs/README.md) · [部署运维](../../ops/README.md) · [配置参考](../../ops/config/README.md) · [备份与恢复](../../ops/deploy/RECOVERY.md)

Auction 的运维命令行：配置与部署预检、分体 systemd 部署的顺序启停、数据库迁移/建库/备份恢复/重建，以及凭据、规则、论坛、装备 PM、邮件证据、Bot 资金和结算异常等显式运维入口。

它只装配既有的配置 loader、应用端口与存储实现，不绕过领域规则、幂等账本或追加式审计；普通服务启动不会隐式迁移、备份或删库。显式维护使用本工具；合并部署另允许在首次设置网页确认后初始化专用空库。参数的权威说明是 `auction-cli <命令> --help`。

## 构建

| feature | 作用 |
| --- | --- |
| （默认无） | 可用 `config show/check` 与 `service status/stop`；数据库、备份、业务命令和 `service start/restart` 明确报错 |
| `postgres` | 数据库、备份恢复与全部业务运维命令 |
| `cache-redis` | 隐含 `postgres`；规则导入后失效 Redis 中的 Public 规则缓存 |

```bash
cargo build --locked --release -p auction-cli --features postgres,cache-redis
```

发布包中的 `bin/auction-cli` 已按上述 feature 构建（见[发布流程](../../ci/README.md)）。仅支持 Linux x86_64。

## 通用约定

- **配置目录**：全局参数 `--config-dir <DIR>`（默认 `/etc/auction`）指向含 `public.toml`、`admin.toml`、`worker.toml` 的目录，分体部署与合并部署（`auction-server` 的 `--config-dir`）使用同一布局。相对路径按当前目录解析。
- **数据库来源**：数据库、凭据、邮件、资金与结算命令统一读取 `worker.toml` 及其敏感文件中的 `[database]`；三角色指向同一数据库由 `config check` 保证。连接串、口令、Cookie 与数据密钥不会出现在参数、输出或日志中。
- **输出**：成功的结构化结果以单个 JSON 文档写入 stdout；进度与诊断写入 stderr，进度前缀 `[auction-cli]`，顶层错误前缀 `auction-cli:`。失败可能没有 JSON，或已先输出状态 JSON；脚本应结合退出码判断结果，不要解析错误文本。
- **文件权限**：进程以 `umask 077` 运行；证据、回执、备份与 manifest 以 `create_new` + `0600` 写入，父目录须为非符号链接、禁止 group/other 写入，已存在的目标一律拒绝覆盖，并在报告成功前同步文件与父目录。

### 退出码

| 退出码 | 含义 |
| --- | --- |
| `0` | 成功（`--help`、`--version` 同样为 0） |
| `2` | 参数错误，或 `config` 命令失败 |
| `3` | `db`、`backup` 命令失败；定时备份的配置、水位、锁、超时、创建、校验、异机副本或清理失败也使用此码，打卡失败不改变退出码 |
| `4` | `service` 命令失败或部署未完全就绪 |
| `5` | 业务运维命令失败、被拒绝，或已提交但需要人工处理（如资金账户冻结） |

## 命令一览

| 命令组 | 子命令 | 是否写入 |
| --- | --- | --- |
| `config` | `show <role>`、`check [--strict]` | 只读文件 |
| `service` | `status`、`start`、`stop`、`restart` | 启停 systemd unit；start/restart 执行迁移 |
| `db` | `status`、`migrate`、`create`、`preflight`、`inventory`、`rebuild` | migrate/create/rebuild 写入 |
| `backup` | `create`、`verify`、`restore`、`scheduled` | restore 写数据库；scheduled 写本地备份/回执并按资格清理，可选 age 加密/R2 副本与外部打卡 |
| `credential` | `list`、`import` | import 写入 |
| `rules` | `import` | 写入 |
| `forum` | `recovery`、`authorize-retry` | authorize-retry 写入 |
| `pm` | `list`、`reconcile` | reconcile 写入 |
| `mail` | `export-evidence`、`inspect-sent` | 导出只读本地归档；Sent 核查还读取数据库与远端邮件，结果写入本地文件 |
| `funds` | `preview`、`open-record-only`、`reconcile` | 后两者写入 |
| `settlement` | `equipment-loss <LOT_UUID>`、`confirm-sent <DELIVERY_UUID> --evidence <JSON>` | 默认只读；`--execute` 记录相应人工处置或 Sent 证明 |

## 配置检查

```bash
auction-cli --config-dir /etc/auction config show worker
auction-cli --config-dir /etc/auction config check --strict
```

`config show` 加载单个角色并输出有效配置的安全投影：文件路径、监听、数据库后端、缓存、可观测性开关、HTTP 安全、会话、拍卖模板摘要、Worker 子系统开关与密钥“是否配置”，不输出任何秘密值。

`config check` 联合加载三角色，核对：

- 三个角色文件、`common_file`、`secrets_file`、`runtime_config_file` 均为非符号链接普通文件；三角色引用同一 `common_file`，并同时引用同一 `runtime_config_file` 或全部省略。
- 三角色数据库为同一 PostgreSQL 目标（主机/端口/库名，拒绝隐式目标、未知参数与 `options`），`equipment_data_key` 标识与材料一致。
- Admin 与 Worker 的有效拍卖模板/排期、Worker control socket、`user_login_data_key`、`credential_data_key` 一致；Admin 若配置 `worker_config_file`，必须指向本目录的 `worker.toml`。
- 三角色加载到的装备规则与 `bot_players` 名单一致（runtime 文件被并发替换时会失败，稳定后重试）。
- Worker 的备份策略范围与可选打卡 HTTPS 地址合法；报告中的 `backup` 给出普通策略，`backup_heartbeat_success_configured/backup_heartbeat_failure_configured` 只给是否配置。`config show` 的 `backup` 投影也包含策略与地址配置标志，不输出地址。
- Public/Admin 监听不使用端口 0、互不重叠，Unix listener 不复用 Worker control socket。
- 权限：配置目录、角色/公共文件禁止 group/other 写入；敏感文件仅 owner 可访问。文件类型与权限复用同一次元数据读取，避免重复 stat。这类检查在非严格模式的 JSON `warnings` 中报告，`--strict` 直接失败。runtime 文件必须为 `0640` 且父目录禁止他人写入，否则 loader 即拒绝，非严格模式也无法绕过；生产部署门禁必须使用 `--strict`。

## 分体部署编排

`service` 只用于分体 systemd 部署（三个角色各自一个 unit）。合并部署（`auction-server`）由 supervisor 自行编排；`service start/restart` 发现该配置目录对应的 `owner.lock` 时拒绝执行。`status/stop` 仍查询或停止所指定的分体 unit，不能用于管理 supervisor。

```bash
auction-cli --config-dir /etc/auction service status
auction-cli --config-dir /etc/auction service start --startup-timeout-seconds 30
auction-cli --config-dir /etc/auction service restart
auction-cli --config-dir /etc/auction service stop
```

| 参数 | 默认值 |
| --- | --- |
| `--systemctl` | `/usr/bin/systemctl` |
| `--state-dir`（部署控制锁目录） | `/var/lib/auction/deployment` |
| `--worker-unit` / `--admin-unit` / `--public-unit` | `auction-worker.service` / `auction-admin-api.service` / `auction-public-api.service` |
| `start/restart --startup-timeout-seconds` | 每角色 readiness 的完整预算（状态查询、探测、进程身份复核及轮询等待），默认 30 秒，范围 1–300；`systemctl start` 本身另有 90 秒上限 |

- **start**：取得部署锁（`<state-dir>/deployment-control.lock`，目录不存在时以 0700 创建）→ `config check --strict` → 核对三个 unit 均已装载、`Environment=AUCTION_CONFIG_FILE` 与 `--config-dir` 中的角色文件是同一文件、Admin runtime 文件与父目录归 Admin unit 的固定 `User` 所有（不接受 `DynamicUser`）→ 三角色全部运行时只按顺序复核 readiness 并返回 `already_running`；部分运行或处于转换状态则拒绝 → 检查 TCP 监听端口可绑定 → 执行迁移 → 按 Worker → Admin → Public 逐个 `systemctl start`，等待 unit 为 active 且应用 readiness 返回 HTTP 200，探测前后 `MainPID` 与 `InvocationID` 不变。任一角色失败时反序停止本次启动的全部 unit；已提交的迁移不会回退。
- **readiness**：Public/Admin 探测 `GET /health/ready`（TCP 或 Unix socket），Worker 探测 control socket 的 `GET /v1/health/ready`。Worker control 只接受 `allowed_peer_uids/gids` 中的调用者，**以 root 运行 `service` 时须把 `0` 加入 `allowed_peer_uids`**，否则 Worker 永远无法判定就绪；因此 `service start` 也要求启用 `[worker.control]`。
- **restart**：先完成与 start 相同的预检（新配置不可用时不停止现有服务），再按 Public → Admin → Worker 停止，随后迁移并顺序启动。
- **stop**：按 Public → Admin → Worker 逐个停止，并确认每个 unit 已停止后才处理下一个。
- **status**：不取锁、不要求严格权限；输出每个 unit 的 `load_state/active_state/sub_state/main_pid` 与 readiness，任一角色未就绪即以 4 退出。配置无法加载时仍报告 systemd 状态，并在 `config_error` 中给出原因。
- systemctl 调用均有超时（查询 10 秒、启动 90 秒、停止 120 秒）；超时只终止 systemctl 客户端，已提交给 systemd 的 job 可能仍在执行，须重新查看状态。

[`ops/deploy/systemd/auction-stack.service.example`](../../ops/deploy/systemd/auction-stack.service.example) 把 `service start/stop` 包装为 oneshot unit，用于开机按顺序拉起。

## 停机门禁

`db migrate`、`db create`、`db rebuild` 与 `backup restore` 在写数据库前取得停机证明：

1. 若 `<config-dir>` 的同级运行目录 `.<目录名>.server/owner.lock` 存在（合并部署），以非阻塞方式取得其独占锁；`auction-server` 或其子进程仍在运行时失败。合并部署必须用与 `auction-server` 相同的服务账号执行，锁文件须为该账号所有的单链接 owner-only 文件。
2. 否则先取得部署控制锁（`--state-dir` 不存在时以 0700 创建），再查询三个 systemd unit，全部为 `inactive`、`failed` 或 `not-found` 才继续；目录首次创建期间也与 `service` 命令互斥，目录或锁无法安全创建时拒绝维护。
3. `--no-systemd` 显式声明本机不用 systemd 管理三角色（容器或开发环境），跳过第 2 步；合并部署锁检查仍然生效。

这些参数与 `service` 相同（`--systemctl`、`--state-dir`、`--*-unit`）。门禁只能证明进程已停止，不能阻止外部客户端连接数据库；恢复与重建另有“零其他会话”检查。

## 数据库

### 状态与迁移

```bash
auction-cli db status
auction-cli db migrate
```

`db status` 只读输出目标库身份、服务器版本、是否只读/recovery、其他会话数、库大小、schema 身份与迁移历史；schema 不是当前二进制的期望版本时以 3 退出。

`db migrate` 在门禁后执行二进制内嵌的全部追加式迁移（SQLx 迁移锁内），输出迁移前后版本。空库会建立完整 schema；存在未知用户对象的非空库被拒绝；历史 checksum 不匹配、失败记录或缺失版本同样失败。迁移只向前，不提供回滚。

### 建库

```bash
auction-cli db create --database auction --maintenance-secrets /etc/auction/maintenance/postgres.toml
```

首次部署时用独立维护凭据创建应用数据库：`--database` 须与 Worker 配置的库名一致，owner 为 Worker 配置中的数据库账号。维护凭据文件（模板见 `ops/config/examples/secrets/postgres-rebuild.toml`）使用 `[database]` 五字段，也兼容与之互斥的 `database_url`；维护库名固定为 `postgres`，主机与端口须与应用库相同。目标提取复用实际 SQLx 连接选项：`hostaddr` 按查询顺序覆盖 TCP host，已指定的 Unix socket 优先于后续 TCP host；socket 路径区分大小写，IP 地址按规范化地址比较。文件必须是规范化绝对路径、非符号链接、单硬链接、归当前账号所有且仅 owner 可读，父目录禁止 group/other 写入。

命令取得集群级 advisory lock，要求连接可写主节点；目标不存在时校验维护角色具备 `CREATEDB`（或 superuser）、应用角色存在且可被指派为 owner，然后 `CREATE DATABASE` 并用应用账号回连验证。目标已存在时只校验 owner 与应用连接，返回 `already_exists`。本命令不迁移、不修改已有库，随后需执行 `db migrate`。

### 恢复预检与保留盘点

```bash
auction-cli db preflight --database auction --output /var/backups/auction/preflight-2026-10-30.json
auction-cli db inventory --database auction --output /var/backups/auction/inventory-2026-10-30.json
```

两者都在只读、可重复读事务中执行，`--database` 必须与连接实际的 `current_database()` 一致，完整证据写入 `--output`，stdout 只给摘要。

- `preflight`：报告是否可写主节点、其他会话数、用户对象数、schema 身份与迁移历史；`restore_preflight_passed=true` 只表示“空库、无其他连接、可写主节点”，不代表任何备份可用。
- `inventory`：读取迁移/schema 身份、public 关系规模与外键、realtime 水位，并对照编译期内嵌的[保留政策](../../ops/deploy/retention-policy.v1.json)输出各数据族统计与政策 SHA-256。`eligibility_assessed`/`eligibility_authorized` 恒为 false，结果不授权任何归档或删除。政策的 `database_schema_version` 必须等于当前最高迁移版本，新增迁移时须同步审阅该政策。

### 破坏性重建

```bash
# 只读演练（默认）：持锁预检并输出预检证据
auction-cli db rebuild --database auction \
  --maintenance-secrets /etc/auction/maintenance/postgres.toml \
  --mode empty-for-restore --backup-manifest /var/backups/auction/auction-20261030.dump.manifest

# 实际执行
auction-cli db rebuild --database auction \
  --maintenance-secrets /etc/auction/maintenance/postgres.toml \
  --mode empty-for-restore --backup-manifest /var/backups/auction/auction-20261030.dump.manifest \
  --execute --confirm 'REBUILD auction' --receipt /var/backups/auction/rebuild-20261030.json
```

- 必须提供已通过校验的 `--backup-manifest`，或用 `--ephemeral` 明确声明目标是可丢弃的一次性库（此时确认短语为 `EPHEMERAL <库名>`）。`--mode empty-for-restore` 强制要求备份，并在 DROP 前复用 `backup restore` 的版本门禁，拒绝高于当前二进制支持版本的备份；通过后只建立空库等待恢复。`--mode fresh-schema`（默认）建库后立即执行全部迁移。
- 维护连接取得与建库相同的 advisory lock，并要求：维护库为 `postgres`、PostgreSQL 主版本 18、可写主节点、目标允许连接且非模板、无自定义 ACL 或库级设置、UTF8 编码、维护角色为 superuser 或“目标 owner + CREATEDB”、目标零活动会话、无预备事务、无活动逻辑复制槽与订阅。
- 执行时在 DROP 前复核恢复点与目标身份，以原 owner、编码、locale provider、tablespace 与连接上限 `DROP DATABASE … WITH (FORCE)` 后 `CREATE DATABASE … TEMPLATE template0`。
- 一旦进入 DROP，无论成败都写入 `--receipt`，记录 `database_dropped`/`database_recreated`（`null` 表示远端结果未知）。未知结果不能推断为未执行，禁止盲目重试 DROP；保持全部角色停止并人工核对。

## 备份与恢复

```bash
auction-cli backup create --database auction --output /var/backups/auction/auction-20261030.dump
auction-cli backup verify --manifest /var/backups/auction/auction-20261030.dump.manifest --database auction
auction-cli backup restore --manifest /var/backups/auction/auction-20261030.dump.manifest --database auction --dry-run
auction-cli backup restore --manifest /var/backups/auction/auction-20261030.dump.manifest --database auction \
  --receipt /var/backups/auction/restore-20261030.json
```

- 依赖部署环境 `PATH` 中的 `pg_dump`/`pg_restore`，主版本不得低于服务器（PostgreSQL 18）。连接参数投影复用实际 SQLx 连接选项的有效 host/socket、port、database 与 user；仅传最终 `PGHOST`，不独立保留 `PGHOSTADDR`，避免预检与外部工具因查询覆盖规则不同而连接不同实例。继承的连接相关 `PG*` 环境全部清除，显式口令只写入 0600 临时 passfile 并在命令结束后删除，从不进入进程参数；TLS 与显式口令仍从敏感配置提取，不承诺与 SQLx 的全部环境默认值或隐式凭据来源一致。连接串只能含 `host`、`hostaddr`、`port`、`dbname`、`user`、`password`、`sslmode`、`sslrootcert`、`sslcert`、`sslkey`、`application_name`（另兼容 SQLx 的 TLS 别名与 `statement-cache-capacity`），`options` 与未知参数被拒绝。
- `create`：`--output` 必须以 `.dump` 结尾，文件名不得含反斜杠、控制字符或首尾空白，archive 与同目录 `<archive>.manifest` 均不得已存在。备份源须有 Auction schema 身份与完整迁移历史；`pg_dump --format=custom --no-owner --no-privileges` 写入临时文件，经 `pg_restore --list` 解析并同步 archive 文件后以硬链接无覆盖发布，随后同步父目录并写入 manifest；备份前后 schema 版本不一致则丢弃 archive。
- manifest 格式为 `auction-postgres-backup.v1`，记录 archive 文件名、archive SHA-256、库名 SHA-256、schema 版本与创建时间；此前运维工具生成的同格式备份可直接校验与恢复。
- `verify`：不连接数据库，校验 manifest 字段闭集、库名摘要、archive SHA-256 与 archive 目录。
- `restore`：通过停机门禁；备份 schema 版本不得高于当前二进制；目标必须是同名空库（零其他会话、可写主节点、无用户对象、无 schema 身份与迁移历史，可先用 `db rebuild --mode empty-for-restore` 准备）。恢复前再次校验恢复点，以 `pg_restore --single-transaction --exit-on-error --no-owner --no-privileges` 执行，完成后核对 schema 身份与迁移版本等于备份版本，再写入 `auction-postgres-restore.v1` 回执。恢复较旧备份后按提示执行 `db migrate`。
- 恢复不保留对象 owner 与权限（以应用账号为 owner 的空库恢复后即归应用账号所有）。

完整的灾备流程见[备份与恢复手册](../../ops/deploy/RECOVERY.md)。

### 定时备份

```bash
auction-cli --config-dir /etc/auction backup scheduled --database auction \
  --output-dir /var/backups/auction/scheduled --state-dir /var/lib/auction/backup
```

三个参数均必填。输出与状态目录必须为不含 `..`、无符号链接的绝对路径，彼此不重叠，归执行账号所有且为 0700；缺失时创建。用固定的执行账号、输出目录与状态目录，避免把一套备份的清理证据拆散。运行账号须能读取 Worker 角色文件及其引用的 common、敏感与可选 runtime 文件，并在 PATH 中提供 PG18 客户端工具；启用异机副本时还须提供主机 `age`，服务器只放收件人公钥。

命令先取得状态目录独占锁，再收口残留运行记录；取锁失败只写 stderr 并退出 3，不创建运行记录或回执。持锁后初始化本次运行记录，调用与手动 create 相同的 Worker loader，再执行 `validate_backup()`；不联合加载 Public/Admin。随后在总预算内取得输出目录锁，检查磁盘 →（启用异机时 age 版本预检）→ create → 再检查磁盘 → verify →（本次加密及日/月副本、逐项重试旧项）→ 本地保留清理 → 发布终态回执 → 可选打卡。create 消费同一次加载得到的 Worker 快照。备份只读取数据库，不停服、不迁移、不执行恢复。本地策略和可选 `[backup.offsite]` 见[配置参考](../../ops/config/README.md#定时备份策略)。手动 create/verify/restore 的参数与 stdout 保持原契约。

archive 命名为 `scheduled-<UTC秒时间>-<run_id>.dump`。本次新备份通过 verify 及已启用的副本阶段后，保留它和最近 N−1 份符合资格的旧备份：旧备份须在同一输出目录、有匹配库名摘要的 `completed` 回执、命名与运行 ID 一致、曾通过校验，且设备号/inode/大小与回执一致。文件必须是当前账号所有的 owner-only 普通单链接文件。删除前只复核这些证据和文件元数据，不重新读取旧 archive 或执行 verify；先删 manifest，再删 archive。本地校验前失败的运行回执（无 archive 或 verified 为 false）静默忽略，不记副本异常；已通过本地校验但副本资格不足时才记 `replica_not_verified`，副本证据不可读或无效时记 `replica_evidence_invalid`。实际目录项异常或无完成证明时仍记入 `cleanup.skipped` 并保留；列表最多记录 256 项，另有完整计数。手动备份和不完整备份不会自动删除。该次启用了异机副本时还要求 daily 完成证据；旧 v1 回执缺省为未启用，保持原清理资格。旧 failed/interrupted 项重试成功时追加 `offsite/completed/<run_id>.json`（`auction-backup-offsite.v1`），绑定原库名、输出目录和明文 manifest 摘要；清理可以使用这个事实，但不改写原运行终态。`offsite-staging/` 子目录不进入清理枚举或跳过噪音。

异机副本启用前必须完成[前置准备](../../ops/deploy/RECOVERY.md#异机副本前置准备)。`age --version` 预检失败发生在 pg_dump 之前。verify 后先在 `<state-dir>/offsite/pending/<run_id>.json` 保存任务身份，再分别用 `age --encrypt --recipient … --output <私有临时文件> <输入>` 加密 archive 和 manifest，stdin 关闭，stderr 有界捕获且不进入日志或回执，子进程取消时终止。密文位于同文件系统的 `<output-dir>/offsite-staging/<run_id>/<archive>.age`、`<archive>.manifest.age`；0600 临时文件 fsync 后无覆盖封存。同一任务永远复用原密文；封存后、checkpoint 前中断也会认领原字节，不重新加密。加密前预留明文大小、每 64 KiB 的 16 字节标签及每文件 16 KiB 头部余量，已暂存数据直接计入文件系统占用。

本次新副本优先处理，然后按开始时间重试旧待上传项；有限预算优先取得最新恢复点，不被积压饿死。重试共用总预算，检查目的地、公钥绑定、文件身份和摘要。无效、不可读、原回执不匹配或目的地已变更的旧任务跳过并保留原文件和密文，在 `offsite.skipped_pending` 记录固定原因码告警；这些跳过告警本身不使本次失败。单个项目上传、冲突或收口失败记录到 `offsite.failed_items`，继续其他项目；全部处理完或总预算耗尽时，只要有失败，本次 outcome 为 failed、退出 3，failure 保留首个失败的阶段与固定错误码。正在执行时 outcome 保持 running，预算耗尽将活动项目记为 timeout，且不覆盖更早的失败；状态目录整体无法枚举或运行记录无法写入时仍停止推进。任务持久保存原密文身份、摘要、已校验对象及固定字节的完成记录；上传失败退出 3，保留本地备份、密文和任务。daily 和所需 monthly 完成后追加本地完成事实，删除暂存密文与待上传项。长期失败会使本地积累直到 `disk_low`，不通过删除尚未取得副本资格的备份腾空间。

对象路径为 `<prefix>/daily/YYYY/MM/DD/<run_id>/<文件名>`，日期取 manifest `created_at` 的 UTC 日期。每个目的地、每个备份 UTC 月的第一份完成副本还把相同密文写入 `monthly/YYYY/MM/<run_id>/`。月标记保存在 `<state-dir>/offsite/months/`；缺失时遍历 LIST 的全部月完成记录，格式、字段、缺件或长度/摘要无效的逐条跳过，只输出固定原因码 `monthly_record_invalid`。遇到首份完整有效记录即恢复标记；全部无效则正常创建本次月副本，LIST/GET 传输或权限错误仍使该项目失败。旧任务先前的未完成月选择会复核，避免新副本已完成后再次发布另一份月完成记录。先上传 archive、再 manifest，最后发布 `completed.json`（`auction-backup-upload.v1`：run_id、created_at、两份对象名/长度/密文 SHA-256/状态、明文 manifest SHA-256、age 版本、校验时间）；没有完成记录的副本不是成功恢复点。单次 PUT 原生 create-only；超过 128 MiB 用 64 MiB 起的分片、最多 10000 parts、并发最多 2。分片先 HEAD，其完成请求没有条件写入，依靠 bucket lock、唯一命名、原密文重用与完整校验保护不可变性。409/412 或响应丢失后，已有对象长度和完整 SHA-256 一致记 `already_present`，不一致为冲突；不覆盖、删除或换名。所有对象均 HEAD + 顺序区间回读校验；每个 get_range 不超过 64 MiB，累计实际长度和全部字节的 SHA-256，两个结果均匹配才承认成功。每段受 300 秒请求超时约束，整体仍受本次运行预算约束，不把 ETag 当摘要；工具不删除远端对象，淘汰由 bucket 生命周期完成。

磁盘使用备份目录文件系统的非特权可用块计算：字节与比例下限都须满足，任一次不足记录 `disk_low` 并退出 3，不先清理仍应保留的恢复点。总预算从命令启动计算，异步子进程、数据库等待、分块摘要读取和清理共享期限；到期记录 `timeout` 并退出 3；若此前已有项目失败，顶层 failure 保留首错，超时活动项目另记在 `offsite.failed_items`。终态同步及最多五秒的打卡另留 systemd 余量。`SIGKILL`、断电或 systemd 强制终止当次可能只留下运行中记录，故当次依赖 systemd 失败状态与 journal；下次启动取得同一状态目录锁后，把有效残留记录收口为 `interrupted`。若终态已发布、身份与已知 archive 证据一致，则沿用已有终态，不改写结果。残留记录无法解析、字段无效，或同名终态无效/冲突时，将原文件以 0600、无覆盖改名为 `<state-dir>/running.invalid-<UTC时间戳>-<本次run_id>.json`，记录固定告警码后照常备份；历史终态不修改。只有记录读写或改名保留等存储操作失败才记 `recovery_failed` 并停止。

终态文件为 `<state-dir>/receipts/<run_id>.json`，格式 `auction-backup-run.v1`，0600 临时文件写入并 fsync 后以 `RENAME_NOREPLACE` 无覆盖发布，再同步目录；历史终态不覆盖。运行中状态只有 `<state-dir>/running.json`，阶段更新以临时文件原子替换；`<state-dir>/scheduled.lock` 从初始化收口到打卡结束始终由当前进程持有，无单独记录租约。运行记录初始化先于配置加载，配置失败也写终态；状态目录本身无法创建或写入时依靠退出码和日志处理。

| 回执字段 | 含义 |
| --- | --- |
| `format/run_id/started_at/finished_at/elapsed_millis` | 格式、身份、UTC 时间与本次运行耗时 |
| `stage_timings` | 已结束阶段的单调时钟耗时；interrupted 保留最后 checkpoint 的已知值，不把停机时间补算成执行时间，finished_at 为收口时间 |
| `outcome/stage/failure` | 终态 `completed/failed/interrupted`；stage 是最后执行阶段，failure 保存首个失败的阶段与固定 `error_code`，不含底层错误链 |
| `database_name_sha256/output_dir/policy` | 库名绑定、受控输出目录、本次普通策略；配置尚未成功时 policy 可为空 |
| `archive` | 文件名、摘要、schema、创建时间、archive/manifest 的本机文件身份和 `verified`；恢复仍使用原 manifest v1 |
| `disk_before/disk_after` | 检查时的总字节、可用字节、`reserved_bytes`（待写密文空间，旧 v1 默认 0）、阈值与是否通过；扣除预留后仍须满足两个水位 |
| `cleanup/recovered_interrupted` | 成功清理及跳过项目和计数、此次收口的中断数；不删除历史回执 |
| `recovery_warnings` | 旧记录无效时为 `running_record_invalid`，同名终态无效或冲突时为 `terminal_receipt_conflict`；只保存固定原因码，不包含文件内容，也不改变新备份结果；旧 v1 回执缺省为 `[]` |
| `heartbeat_configured` | 本次结果对应的成功或失败地址是否配置；不记录 URL 或发送结果，结果只写 stderr/systemd journal |
| `offsite` | `enabled/age_version`、`daily/monthly`（archive/manifest/completion 的 key、bytes、sha256、uploaded/already_present 及 verified_at）、`monthly_written`、`pending_objects`、`retried_runs/retried_count`；重试 ID 列表最多 256 项；旧 v1 缺省未启用 |
| `offsite.skipped_pending/skipped_pending_count` | 跳过旧任务的可选 run_id 与固定 reason；列表最多 256 项，另有完整计数；原因见下文，原任务与密文不改写、不删除 |
| `offsite.failed_items/failed_count/active_run_id` | 项目失败的 run_id、stage、error_code；列表最多 256 项，另有完整计数；active_run_id 为执行中的任务，正常终态为空，旧 v1 字段缺省兼容 |

`offsite.skipped_pending.reason` 为 `pending_unreadable`、`pending_record_invalid`、`pending_destination_changed`、`pending_identity_mismatch`、`pending_source_invalid` 或 `pending_source_mismatch`。告警只含固定原因码与可确认的 UUID，不记录文件内容、秘密或完整路径。当前 daily 即使已经成功，旧项失败仍会使整体 outcome 为 failed；选恢复点应核对 daily 完成证据和远端 completed.json。

常见错误码为 `config_error`、`output_lock_busy_or_invalid`、`output_directory_invalid`、`disk_check_failed`、`disk_low`、`create_failed`、`verify_failed`、`retention_failed`、`recovery_failed`、`receipt_failed`、`timeout`、`interrupted`。正常收口的成功/失败均输出一个 JSON 摘要（`outcome/run_id/receipt/failure`），调用方结合退出码判断；状态目录取锁失败不写回执，回执发布失败可能只保留运行中记录。

异机阶段为 `offsite_preflight/offsite_encrypt/offsite_retry/offsite_daily/offsite_monthly/offsite_finalize`，阶段错误码为 `offsite_preflight_failed/offsite_encryption_failed/offsite_retry_failed/offsite_daily_failed/offsite_monthly_failed/offsite_state_failed`；R2 适配器进一步给出固定分类 `offsite_configuration_invalid/offsite_local_read_failed/offsite_access_denied/offsite_transport_failed/offsite_object_conflict/offsite_remote_record_invalid`。底层 SDK 正文、凭据、签名 URL 和 age stderr 均不输出。回退为 `[backup.offsite] enabled = false`，下一次运行生效，暂停旧待上传项但不放宽其清理资格；停用 timer 停止后续全部定时任务。

打卡读取 Worker 敏感 TOML 的 `backup_heartbeat_success_url/backup_heartbeat_failure_url`，默认空即不建客户端、不联网。外部服务目前暂缓启用，示例保持空值。启用后，在终态发布后向对应地址发送一次 HTTPS GET，禁止 userinfo、重定向、环境代理与重试；连接预算三秒、请求总预算五秒，不携带 Cookie，不读取响应正文。结果只经 `progress()` 向 stderr 输出 `sent/failed/not_configured` 和可选 HTTP 状态码，systemd 下进入 journal，不另写打卡回执，不改变备份结果与退出码。Worker 配置尚不能加载时按未配置处理；回执存储失败则记录错误并尝试一次失败打卡。

## 业务运维

以下命令都需要 `postgres` feature，读取 Worker 配置。

### 凭据

```bash
auction-cli credential list --kind mail --status active
printf '%s' "$COOKIE" | auction-cli credential import --kind mail --world main --label bot-main --actor ops
```

`import` 只从 stdin 读取 Cookie 头（去掉末尾换行），用敏感配置中的 `credential_data_key` 加密后写入，与 Admin 凭据页共用同一服务、命令账本与审计。两种凭据都必须含合法的 `ipb_member_id` 与 `ipb_pass_hash`：`--kind mail`（MoogleMail 游戏 Cookie）只保存这两个字段；`--kind forum`（E-Hentai 论坛 Cookie）保留通过校验的原始 Cookie 头。`--world shared` 同时服务 main 与 isekai。Worker 在启动时装载 active 凭据，导入后需重启 Worker 生效。`list` 只输出安全投影，不含密文或明文。

### 规则导入

```bash
auction-cli rules import --directory /srv/rules --command-id 6f1d…-… --actor ops
```

目录须含 `en.md`、`zh-CN.md`、`zh-TW.md`、`ja.md`、`ko.md` 五个 UTF-8 普通文件：去掉开头单个 BOM，首个非空行去掉前导 `#` 作为标题，正文按 Admin 规则校验。写入前先比对数据库现状：任一语言已存在不同正文即整体拒绝；内容相同报告 `preserved`；缺失语言以 `--command-id` 派生的稳定命令 ID 发布（请求摘要与 Admin 发布接口一致），部分完成后用同一 `--command-id` 重试即可续上。启用 Redis 时随后失效 Public 规则缓存，失效失败列入 `cache_invalidation_failures` 但不回滚已发布规则。

### 论坛写入恢复

```bash
auction-cli forum recovery --limit 100
auction-cli forum authorize-retry --auction-id <拍卖UUID> --command-id <UUID> --actor ops \
  --confirm 'AUTHORIZE FORUM WRITE <拍卖UUID>'
```

`recovery` 只读列出主题写入结果未知、只读对账中或等待人工授权的拍卖。Worker 对未知写入先按精确标题只读查找主题；未命中时停在 `write_authorization_required`，此时才可用 `authorize-retry` 以稳定命令 ID、逐字确认与追加审计授权下一次创建尝试。同一命令 ID 换参数即冲突。

### 装备 PM 对账

```bash
auction-cli pm list --status manual_reconcile
auction-cli pm reconcile --notification-id <UUID> --command-id <UUID> --expected-fencing-version <N> --actor ops
```

`list` 输出超价提醒私信的安全状态、fencing 版本、时间与原发送者，不含正文、收件人或租约 token。`reconcile` 以版本 CAS、稳定命令与追加审计请求 Worker 用原发送者只读核对 Sent；返回的是请求被接受的快照，不表示私信已送达。没有重发、放弃或强制成功入口。

### 邮件证据

```bash
auction-cli mail export-evidence <evidence-id> --output /secure/evidence-<id>.json
auction-cli mail inspect-sent <obligation-id> --output /secure/sent-<id>.json
```

- `export-evidence` 按 Worker 日志中的 `evidence_id` 解密导出 MoogleMail 诊断归档（目录为 `[worker.mooglemail.diagnostics].directory`）。父密钥默认优先 `credential_data_key`、缺失时用 `equipment_data_key`，可用 `--key-source` 指定；轮换密钥后须保留旧密钥才能导出历史证据。不联网、不写数据库。
- `inspect-sent` 只针对撤拍退回（`withdrawn_return`）义务：读取冻结的收件人、世界与装备，用该世界唯一的 active 邮件凭据只读查询 Sent 中的精确邮件，结果标记 `time_unverified`，`collected` 也不等于已送达证明。不发送、不改变履约状态。

### 装备损失人工收口

```bash
# 默认只读，不访问游戏或论坛
auction-cli --config-dir /etc/auction settlement equipment-loss <LOT_UUID>

# 逐字使用 preview 的版本、账户、卖方金额与 confirmation，先替换全部占位值
auction-cli --config-dir /etc/auction settlement equipment-loss <LOT_UUID> --execute \
  --command-id <STABLE_COMMAND_UUID> --actor ops --reason '<装备损失原因及人工处置责任>' \
  --funding-uid <UID> --expected-recovery-version <RECOVERY_V> \
  --expected-payment-version <PAYMENT_V> --expected-delivery-fencing-version <DELIVERY_V> \
  --seller-amount <SELLER_AMOUNT> --buyer-compensation-external \
  --confirm 'RESOLVE EQUIPMENT LOSS <LOT_UUID> <UID> <RECOVERY_V> <PAYMENT_V> <DELIVERY_V> <SELLER_AMOUNT> EXTERNAL BUYER COMPENSATION'
```

- 默认输出 `outcome=preview`，`preview` 原样序列化应用端口的安全投影，包含 `eligible`、`blockers`、资金 UID、恢复/付款履约/投递版本与冻结买卖方金额。`execution.confirm` 给出逐字确认短语，`execution.argv_template` 给出独立参数数组模板；替换配置目录、稳定命令 UUID、操作者与原因占位值后按参数执行，不要把 JSON 直接交给 shell `eval`。没有可投影的资金 UID 时 `execution=null`；有模板不代表资格通过。
- 执行必须同时提供示例中的全部参数。缺少 `--execute` 时不得携带执行参数；缺参数、零 UUID、操作者格式错误或确认短语不匹配时在写入前拒绝。全部业务资格、资金模式与状态版本由结算 owner 在事务中校验；CLI 不另查 SQL 或推断 Sent。
- 本命令仅用于 main 已结算成交、原装备 COD 因库存读取失败而停在人工核对、全部发送尝试明确未开始外写的装备损失。只支持已就绪的 `record_only` 资金账户，`strict_cash` 拒绝执行；账户模式与既有结算金额均不改变。任何外写未知、既有收款或冲突恢复/资产处置都不能靠该命令绕过。
- `--buyer-compensation-external` 是操作者承担买方线下补偿责任的声明，不能证明线下补偿已支付。原付款以 `operator_equipment_loss_v1` 人工处置证明收口，绝不伪造 Sent、COD 收款或卖方已收款；责任资金沿原冻结结算分配，不记为真实 `collected_payment`。原结算与补偿流水保留，不走成交撤销冲正、重列或拉黑。
- 成功输出 `outcome=committed`，同一 ID 同参数重放输出 `outcome=replayed`，`result` 包含原付款投递、卖方义务与金额等审计结果。连接或响应错误不能证明未提交：必须原样重放同一命令 ID 与全部参数，不要先用新 preview 的版本替换原请求；同 ID 换参数冲突。
- 原装备投递永久终结为 `failed/equipment_destroyed`，不能再从 Admin 重试或恢复投递；不再催款或执行该件买方退款。卖方货款只解除阻塞，仍使用原冻结金额，通过正常 `SellerPayout` 履约和真实邮件支付，命令成功不代表卖方已收款。详见[结算与履约](../../docs/guide/settlement.md#装备损失人工处置)。

### Sent 人工证据确认

```bash
# 只读数据库预览；证据由操作者观察原生 Sent 后准备，不发起游戏请求
auction-cli --config-dir /etc/auction settlement confirm-sent <DELIVERY_UUID> \
  --evidence /private/sent-evidence.json

# 逐字使用 preview 的版本、确认短语及要求的接受项
auction-cli --config-dir /etc/auction settlement confirm-sent <DELIVERY_UUID> \
  --evidence /private/sent-evidence.json --execute \
  --command-id <STABLE_COMMAND_UUID> --expected-fencing-version <FENCE> \
  --actor ops --reason '<原生证据来源及人工关联责任>' \
  --accept-collected-assets --accept-attempt-history --confirm '<PREVIEW_CONFIRM>'
```

- 证据只按闭集 `ManualSentEvidence` 解析：`schema_version=1`，`world_id`、`external_mail_id`、`sender_uid`、`sender_name`、`recipient`、`subject`、UTC `sent_minute`/`observed_at`、32 字节数组 `original_body_digest`/`observed_body_digest`、整数 `remaining_credits`/`cod_pending`、字符串数组 `equipment_ids`、布尔 `collected` 和原生 `receipt`。字段全部必填，未知字段拒绝。最多 64 KiB，使用既有有界、非阻塞普通文件读取器，拒绝符号链接、FIFO、设备等特殊文件；不要将凭据放进证据。
- `equipment_ids` 只记录远端当前实际剩余 EID，已领取时为空；不得将期待 EID 填入观察值。原始正文摘要绑定冻结发送正文，不能用当前模板重渲染；已领取时还须保留游戏追加的原生附件收取/COD 回执。应用端口统一核对冻结期待、发件身份、发送分钟区间、金额、附件与历史尝试。
- 入口支持 main 结算的单件装备 COD、流拍/撤拍退回，以及原货款已寄出的 `seller_payout`。原投递须有冻结发送尝试、处于结果未知的人工核对状态或被 Admin 对账判失败且其后无变化（预览 `corrects_admin_failure=true`），且无活跃 claim 或冲突处置。货款只接受正额 Credits、零 COD 与空 EID：未领取时原额仍在且回执为空，已领取时须带原生 `Removed attachment: <千分位金额> Credits` 回执并加 `--accept-collected-assets`；原 `seller/unknown` 资金占用及 reserve/unknown 分录必须精确绑定同一账户、lot、义务、投递和金额。默认 `outcome=preview`，`preview.blockers`/`eligible` 表示当前资格；预览不写入或授权重发。
- 执行参数只能与 `--execute` 同时使用；命令 ID、版本、操作者、原因与 `--confirm` 全部必填。`--accept-collected-assets` 和 `--accept-attempt-history` 只在 preview 要求时加入，示例中的两项不能机械照抄。确认短语绑定投递、版本、MID、类型化证据摘要与两项接受声明，任何内容变化均须重新预览。`execution.argv_template` 是独立参数数组，不得 `eval`。
- 已收取附件后原 EID 无法再自动证明，接受项记录操作者承担的资产关联责任；旧尝试历史不完整时，另一接受项记录人工关联责任。系统保留全部旧尝试、原结果与审计，追加独立不可变确认，以 `sent_operator_confirmed_v1` 区别于 `sent_exact_payload_v1` 自动精确资产证明，不设置 `sent_verified=true` 冒充自动核验。Admin 仍不能仅凭手填 MID 确认主动 Send 成功。
- 成功输出 `outcome=committed`，同 ID 同参数重放输出 `outcome=replayed`。响应或连接错误不能证明未提交，必须保持原命令 ID、证据内容、原因、版本、接受项与确认短语重放；不要用新 preview 的版本替换原请求。同 ID 换参冲突。
- 装备确认只绑定真实 MID、恢复原投递及履约；COD 实收及后续卖方款仍由 Worker 使用真实收取证明与原冻结结算处理。已寄出的货款确认则复用资金 owner，将原 `unknown` 支出记为 `spent`，不创建新付款、收款来源或投递；同命令重放不重复支出。确认均不代表收件人已经领取，领取状态由正常流程继续核验。更正 Admin 失败结论时原失败命令与审计保留，履约从 failed 恢复正常对账；邮件早于当前尝试时须加 `--accept-attempt-history`。详见[结算与履约](../../docs/guide/settlement.md#sent-人工证据确认)。

### Bot 资金

```bash
auction-cli funds preview --uid <UID>
auction-cli funds open-record-only --uid <UID> --command-id <UUID> --actor ops \
  --expected-version 0 --preview-digest <HEX> --confirm '<preview 给出的 open_record_only 模板>'
auction-cli funds reconcile --uid <UID> --command-id <UUID> --actor ops --expected-version <V> \
  --preview-digest <HEX> --proprietary-credits <P> --compensation-credits <C> --dedicated-account \
  --confirm 'RECONCILE BOT FUNDS <UID> <V> <P> <C> <HEX>'
```

Bot 内部履约（买卖双方之一是 Bot 自己时不经邮件外发）需要一个资金账户，见[结算与履约](../../docs/guide/settlement.md#bot-内部履约与资金账户)。

- `preview` 只读数据库，输出账户版本、责任投影、阻断项、`preview_digest`、能否开 record_only 账户，以及两种写入的确认短语模板；不访问远端，不代表任何可用额度。
- `open-record-only`：全局尚无资金账户时，建立只记账（`record_only`）账户——系统记录结算责任与真实收款，不读取余额、不以 Bot 现金限制个人消费。已有账户不能用此命令切换模式。
- `reconcile`：为 `strict_cash` 账户做现金对账，声明本次授权的自有额度与补偿额度；须声明 `--dedicated-account`（专用账号、无并发人工资金写入、无未登记外部负债、全部程序同版）。对账后账户状态不是 `ready`（账实缺口，新增支出冻结）时以 5 退出。
- 写入都先按原命令 ID 重放；之后在 30 秒共享预算内取得与 Worker 相同的邮件/论坛凭据 owner，用 Main 世界唯一的 active 邮件凭据与论坛凭据完成联合身份、干净草稿（以及 strict 模式的现金）观察，再在同一 opaque guard 内提交。因此**必须先停止 Worker**，并启用 `[worker.forum]` 与 `credential_data_key`。
- 输出 `committed`/`replayed` 才算完成；`rolled_back` 为确定回滚；`evidence_unavailable` 表示提交前证据未通过；`outcome_unknown` 表示提交阶段结果未知——保留原命令 ID 与全部参数重新执行，以重放核对，不要换 ID 重试。

凭据、规则与资金命令在连接池建立后，将参数校验、客户端或缓存装配与执行错误统一带回连接池关闭路径；结算损失命令在连接前校验执行声明与确认短语，端口读取、提交、序列化和输出错误也统一带回关闭路径。退出前显式关闭连接池；规则文件复用证据文件的非阻塞普通文件检查与有界读取。

## 源码结构

| 路径 | 职责 |
| --- | --- |
| `apps/auction-cli/src/main.rs` | 入口：平台限定、umask、退出码 |
| `apps/auction-cli/src/cli/` | 命令、参数与默认值 |
| `apps/auction-cli/src/app/` | 分派、退出码、JSON 输出 |
| `apps/auction-cli/src/deployment/` | 配置目录、单角色投影、三角色一致性检查、监听端点 |
| `apps/auction-cli/src/service/` | systemctl 客户端、readiness 探针、编排与停机门禁 |
| `apps/auction-cli/src/database/` | 状态、迁移、建库、预检/盘点证据、重建、连接目标解析 |
| `apps/auction-cli/src/backup/` | manifest、libpq 环境与 passfile、可复用 create、备份/校验/恢复 |
| `apps/auction-cli/src/backup/scheduled/` | 定时编排、目录边界、磁盘门禁、独占锁/回执、保留清理与打卡装配 |
| `apps/auction-cli/src/backup/scheduled/offsite/` | age、同盘暂存、待上传检查点、日/月副本编排与追加完成事实 |
| `apps/auction-cli/src/operations/` | 凭据、规则、论坛、PM、邮件、资金与结算异常 |
| `apps/auction-cli/src/support/` | 私有文件、锁文件与输入校验 |
