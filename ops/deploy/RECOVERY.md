# 备份与恢复手册

[docs/README.md](../../docs/README.md) · [ops/README.md](../README.md) · [apps/auction-cli/README.md](../../apps/auction-cli/README.md) · [docs/contracts/database.md](../../docs/contracts/database.md)

本手册描述 PostgreSQL 数据的备份、校验、恢复与破坏性重建。命令参数的完整说明见 [apps/auction-cli/README.md#备份与恢复](../../apps/auction-cli/README.md#备份与恢复)。所有写数据库的步骤都要求先停止三个角色（停机门禁），并在维护窗口内执行。

## 原则

- **备份 = archive + manifest（+ 可选范围证据）。** `backup create` 生成 `<名称>.dump`（`pg_dump` 自定义格式）与 `<名称>.dump.manifest`（格式 `auction-postgres-backup.v1`：archive SHA-256、库名摘要、schema 版本、创建时间）。启用 `[backup.scope]` 时还生成 `<名称>.dump.scope.bundle.age` 与 `<名称>.dump.scope.json`，四者必须一起保存；范围密文只用离线私钥解密，sidecar 只含摘要与相对路径。manifest 没有数字签名，存储的访问控制与不可变性由部署方负责。
- **恢复只进入空库。** `backup restore` 不删库、不清理 schema、不终止连接，也不会覆盖非空库；需要替换现有库时先用 `db rebuild --mode empty-for-restore` 准备同名空库。
- **破坏性操作必须显式确认并留回执。** `db rebuild` 默认只读演练；只有 `--execute` + 逐字确认短语 + 新回执路径同时成立才会 `DROP DATABASE`。
- **未知结果不能当作未执行。** 回执中 `null` 表示无法确认（例如 DROP 的响应丢失），此时保持停机、人工核对数据库与备份，不要盲目重试。
- 证据文件（archive、manifest、预检、回执）只写入新文件（不覆盖）、权限 0600，父目录必须禁止 group/other 写入。建议使用专用目录：`install -d -m 0700 /var/backups/auction`。

## 例行备份

```bash
auction-cli --config-dir /etc/auction backup create --database auction \
  --output /var/backups/auction/auction-$(date +%Y%m%d-%H%M).dump
auction-cli --config-dir /etc/auction backup verify --database auction \
  --manifest /var/backups/auction/auction-<时间>.dump.manifest
```

- 备份不要求停服（`pg_dump` 读取一致性快照），但若备份期间发生迁移（schema 版本前后不一致），archive 会被丢弃并报错。
- 需要一个“确定的恢复点”（例如升级前）时，先停服再备份：`auction-cli service stop` → `backup create` → 升级 → `service start`。
- 依赖部署环境 `PATH` 中的 `pg_dump`/`pg_restore`，主版本不得低于服务器（18）。
- `backup verify` 不连接数据库，可在异地存储上离线校验。

## 定时备份

发布包含 `systemd/auction-backup.*`、`auction-wal-upload.*` 与 `auction-base-backup.*` 模板；timer 的 `ExecStart` 调用 `backup scheduled`，旧 CLI 必须先升级。以下在解压后的发布包根目录以 root 执行：

```bash
install -d -o auction -g auction -m 0700 /var/backups/auction/scheduled /var/lib/auction/backup
install -m 0644 systemd/auction-backup.service.example /etc/systemd/system/auction-backup.service
install -m 0644 systemd/auction-backup.timer.example /etc/systemd/system/auction-backup.timer
sudo -u auction /opt/auction/bin/auction-cli --config-dir /etc/auction config check --strict
systemctl daemon-reload && systemctl enable --now auction-backup.timer
```

合并部署把 `--config-dir` 改为 `/var/lib/auction/config`。两个目录须私有且不重叠；普通参数在 `common.toml [backup]`（见[定时备份策略](../config/README.md#定时备份策略)），下一次 CLI 调用生效。停用 `systemctl disable --now auction-backup.timer`，正在执行的 oneshot 不自动停止。

timer 选每日 UTC 03:15（随机延迟 ≤15 分钟、CLI 最多两小时），避开默认周五 00:00 开拍、周六 12:00 名义结束与 Final 提前 900 秒的结算高峰；软截止、自定义排期与 `Persistent=true` 补跑可能改变重叠，按实际时段调整。`Persistent=true` 只补一次错过的唤醒。

任务先取状态目录独占锁再收口上次运行，取锁失败只写 stderr 并退出 3。持锁后只加载 Worker 及其引用文件并校验备份策略；总预算默认 7200 秒，create/加密/旧项重试/日与月上传共用，到期写 failed 并退出 3（service 的 130 分钟预算另给终态同步与打卡）。磁盘在 create 前后检查并预留密文空间，暂存目录须与输出同文件系统。

本地保留默认最近 7 份（本次已校验 + 6 份合格旧备份）；清理只认同一输出目录、库名摘要、scheduled 命名与 completed 回执，并检查已校验标志、文件身份与 owner-only 权限；启用异机时还要求已校验的 daily 副本。旧 failed/interrupted 上传项重试成功后追加 `offsite/completed/<run_id>.json`，不改写原回执。异机长期失败导致累积并最终 `disk_low`，属失败关闭；修复上传或扩容，不扩大删除范围。

回执在 `<state-dir>/receipts/<run_id>.json`（`auction-backup-run.v1`，字段见[CLI README](../../apps/auction-cli/README.md#定时备份)），0600、原子发布、终态不可变；运行中为 `<state-dir>/running.json`。强杀/断电可能来不及写终态，下一次运行对残留记录追加 interrupted；无效记录改名保留为 `running.invalid-*.json` 并写入 `recovery_warnings`。

| 回执错误码/现象 | 排查 |
| --- | --- |
| `config_error` | 以 auction 账号 `config check --strict`，检查文件与权限；不要粘贴敏感 TOML |
| 取锁失败 / `output_lock_busy_or_invalid` | 查并发 scheduled 与同一状态/输出目录；不要删锁文件代替核对 |
| `disk_low` / `disk_check_failed` | 检查文件系统与水位；保留恢复点 |
| `create_failed` / `verify_failed` | 查 journal、PG18 客户端与连接条件；新运行保留旧失败记录 |
| `offsite_preflight_failed` | 检查 `age --version` 与执行权限；发生在 pg_dump 前 |
| `offsite_encryption_failed` / `offsite_local_read_failed` | 核对公钥、暂存权限与磁盘；不丢弃已封存密文 |
| `offsite_access_denied` / `offsite_transport_failed` / `offsite_daily_failed` | 检查令牌、endpoint、网络与 bucket lock；下次自动用原密文重试 |
| `offsite_retry_failed` / `offsite_state_failed` | 核对状态目录与 failed_items；状态目录不可写则停止推进 |
| `offsite.skipped_pending` 非空 | 按原因码核对原 pending、回执与密文；不符则保留告警并跳过 |
| `offsite_object_conflict` / `offsite_monthly_failed` / `offsite_remote_record_invalid` | 核对完成记录与密文；禁止覆盖、删除或换名 |
| `retention_failed` | 核对回执、权限与文件身份；工具不删不明文件 |
| `timeout` | 核对备份大小与资源后调整预算；systemd 上限应更长 |
| `recovery_warnings` / `recovery_failed` / `receipt_failed` | 检查 `running.invalid-*.json`、状态/回执目录读写与磁盘；修复后重试 |

外部打卡当前缓启用（地址为空不发请求）。启用后 CLI 先发布终态回执，再向成功/失败地址发一次 HTTPS GET（无 Cookie、不跟随重定向、不读代理、连接三秒总预算五秒、不重试）；回执只存 `heartbeat_configured`，状态只经 stderr 进 journal，失败不改备份结果。

## 数据库之外的备份范围

数据库 archive 只恢复数据；重建一台主机还需要运行配置、邮件诊断证据与部署回执。启用 `[backup.scope]` 后，`backup create` 与 `backup scheduled` 会把这三类材料打包成一个 age 密文，并与数据库恢复点配对：

| 文件 | 内容 |
| --- | --- |
| `<archive>.scope.bundle.age` | age 公钥加密的打包文件；格式 `auction-backup-scope-bundle.v1`，逐条记录绑定相对路径、字节数与内容摘要 |
| `<archive>.scope.json` | 格式 `auction-backup-scope.v1` 的公开 sidecar：密文/明文摘要、字节数、文件数、来源类型与最多 256 条文件相对路径；不含绝对来源目录、公钥或文件内容 |

规则与边界：

- 运行配置只纳入本轮实际加载到的文件（角色、`common_file`、敏感文件、可选 runtime 文件），不递归配置目录；owner 凭据、维护凭据与同目录其它文件不会被自动带走。
- 邮件诊断证据来自 Worker `[mooglemail.diagnostics].directory`，目录尚不存在时按空集合记录；诊断文件本身已是认证加密的证据格式。
- 部署回执由 `auction-cli service start/restart` 追加到 `<state-dir>/receipts/<receipt_id>.json`（格式 `auction-deployment-receipt.v1`），并由 `[backup.scope].deployment_receipts_dir` 指向该目录纳入范围；目录必须存在且为绝对路径。
- 范围内容一律加密，即使异机副本关闭也一样；因此启用范围时 `age_recipient` 必填，且 PATH 中必须有 `age`。明文只存在于输出目录内的 0600 临时文件，封存后被主动删除；创建中途被杀可能留下 `.partial`，人工确认后删除，不能把它当作恢复材料。
- 超过 `max_files`、`maximum_total_bytes`、单文件 64 MiB、符号链接、特殊文件、非 UTF-8 名称或控制字符时失败关闭，不做部分范围备份；失败阶段为 `scope`、错误码为 `scope_failed`，此时 archive/manifest 已存在但缺少 sidecar，不能当作完整恢复点。
- 启用异机副本时，daily/monthly 的完成记录各自带 `scope`（密文与 sidecar 两个对象）；sidecar 最后上传，便于下载侧先核对摘要。旧待上传项没有范围对象时照常完成。
- 本地保留清理只在回执的 `archive.scope` 与本机文件身份一致时删除这两个文件；身份不符或删除失败时保留并记录 `remove_failed`。

恢复校验：

```bash
# 离线或隔离主机；identity 是离线托管的 age 私钥
auction-cli backup scope-extract \
  --manifest /srv/restore/scheduled-<时间>-<run_id>.dump.manifest \
  --database <原库名> \
  --identity /offline/age-identity.txt \
  --output-dir /srv/restore/scope-<时间> \
  --receipt /srv/restore/scope-<时间>.json
```

命令先执行与 `backup restore` 相同的恢复点校验（archive、manifest 与范围 sidecar），把密文解密到输出目录内的私有临时文件，核对整体明文摘要、文件数与 sidecar 逐条摘要，全部通过后才按 `runtime_config/`、`mail_diagnostics/`、`deployment_receipts/` 写入每个文件。目标目录必须不存在或为空，文件以 0600 新建、不覆盖；临时明文在校验后删除。输出的回执只含摘要、来源类型、文件数与目录，不含文件内容或绝对来源路径。

人工核对：把解出的配置与当前部署逐项比对（连接目标、监听、密钥“是否配置”、运行时开关），确认部署回执中的程序摘要、迁移版本、恢复点标识与时间同实际部署一致（用 `auction-cli service receipt list/show` 对照）；邮件诊断证据只在需要排查投递时按原加密格式读取。解出的运行配置含数据库口令、R2 令牌与邮件凭据，只能留在离线或隔离主机，不得进入日志、ticket、聊天或 Git。恢复数据库仍按[恢复流程](#恢复流程)执行，不能用范围材料替代 archive 恢复或外部历史核对。

已闭环：月度恢复演练可把 `backup restore` 的 `auction-postgres-restore.v1` 回执（或通过的迁移演练回执）复制到 `alert report --restore-drill-dir` 目录，由 `restore_drill_overdue` 检查是否超过 35 天。仍未闭环：恢复后外部核验的端到端演练（`recovery external-reconcile` 工具已存在，尚未用真实备份完成演练）与部署自动引用回执的发布门禁。当前只有手工 `scope-extract` 与 `backup verify` 的摘要校验，不能据此声明端到端演练完成。

## WAL 归档与时间点恢复

目标：RPO ≤ 5 分钟（`archive_timeout = 300`）、RTO ≤ 2 小时，可恢复到过去 14 天内任一时刻；WAL 与基础备份保留 21 天，同时保有 3 份基础备份。R2 使用独立前缀 `<prefix>/wal/` 与 `<prefix>/base/`，生命周期与 bucket lock 均为 21 天，与日/月备份互不影响。当前交付 `archive_command` 调用的 `wal archive`（zstd + age + fsync 暂存）、每周 `wal base-backup`（`pg_basebackup` 物理备份）与 `wal upload`（R2 上传、长度与完整 SHA-256 校验）；WAL 校验后删本地副本，基础备份本地保留 3 份。归档、上传与暂存指标和外部打卡尚未实现，靠 `pg_stat_archiver`、systemd 状态与磁盘观察。

### 组成与数据流

1. archiver 在段写满或 `archive_timeout` 到期后执行（postgres 账号，不读敏感 TOML；公钥写在参数中）：

   ```conf
   archive_command = '/opt/auction/bin/auction-cli wal archive --staging-dir /var/lib/auction/wal-staging --recipient age1<收件人公钥> %p %f'
   ```

   命令把 zstd 流经 `age --encrypt` 写入 `<段名>.zst.age`，fsync 文件与目录后发布 `<段名>.zst.age.manifest.json`（`auction-wal-archive.v1`）。失败返回非零由 PostgreSQL 重试；同段源字节数与 mtime 一致时直接成功，不重复加密；源内容或密文长度不符时失败关闭。

2. `auction-base-backup.timer` 每周以 postgres 账号执行 `wal base-backup`，运行 `pg_basebackup --pgdata=- --format=tar --wal-method=fetch --checkpoint=fast --no-password`（本机 socket、peer 认证；账号须有 `REPLICATION` 或超级用户；集群不能有额外表空间），tar 流经 zstd + age 写入 `base-<UTC时刻>.tar.zst.age` 并发布 `auction-base-backup.v1` manifest（备份 ID、起止时刻、压缩前/密文字节与 SHA-256）。外部程序非零即失败关闭，不发布半成品。

3. `auction-wal-upload.timer` 每 5 分钟以 auction 账号运行 `wal upload --budget-seconds 300`：WAL 按段名上传，校验后先写 `<段名>.wal-upload.json` 证据再删本地密文与 manifest；基础备份上传到 `<prefix>/base/<备份 ID>/`，顺序为密文、manifest、`completed.json`（`auction-base-upload.v1`），完成记录取备份完成时刻，响应丢失后重试生成相同字节。远端同长度与完整 SHA-256 的对象视为成功（bucket lock 下重传 409）。单份密文无法在预算内上传时同时调大 `--budget-seconds` 与 service `TimeoutStartSec`。

4. 基础备份上传成功后写本机 `base-<ID>.upload.json`（`auction-base-receipt.v1`），后续运行在目标身份一致时跳过重算与远端回读；证据缺失、无效或目标变化时按未上传处理。每次运行末尾按备份 ID 倒序保留最新 3 组完整暂存项，删除更旧组的本机证据、manifest 与密文；上传失败的旧项不删除，不完整的暂存项只报告 `incomplete`。异机关闭时保留策略照常执行，报告 `outcome = skipped`。

5. 上传失败、冲突或预算耗尽保留本地文件并非零退出；JSON 报告逐项给出 `uploaded`、`already_present`、`failed`、`invalid`、`incomplete`、`remaining` 与 `base.*` 计数。

### 启用顺序

1. 安装包含 WAL 子命令的 `auction-cli`。
2. 在 postgres 账号 PATH 中安装 `zstd`、`age`、`pg_basebackup`（PG18），确认可执行且可运行 `/opt/auction/bin/auction-cli`。
3. 建暂存目录 `postgres:auction 2770`（setgid），例如 tmpfiles 的 `d /var/lib/auction/wal-staging 2770 postgres auction - -`。
4. 以 auction 账号 `config check --strict`，确认 `[backup.offsite]` 已启用且令牌可写两个前缀。
5. 手工验证归档与上传（不改 `archive_mode`）：

   ```bash
   sudo -u postgres /opt/auction/bin/auction-cli wal archive \
     --staging-dir /var/lib/auction/wal-staging --recipient age1<收件人公钥> \
     /path/to/pg_wal/000000010000000000000001 000000010000000000000001
   sudo -u auction /opt/auction/bin/auction-cli --config-dir /etc/auction wal upload \
     --staging-dir /var/lib/auction/wal-staging --budget-seconds 300
   ```

   重跑归档须输出 `already_archived` 并退出 0；重跑上传须报 `already_present` 或空集合。
6. 手工验证基础备份与上传：`sudo -u postgres … wal base-backup …` 输出 `status = created`，上传报告 `base.uploaded` 或 `base.already_present`，R2 `<prefix>/base/<ID>/` 三份对象齐全。同一天再次运行会创建新 ID，保留策略只在超过 3 份时删除最旧完整副本。
7. 安装并启用 `auction-wal-upload.*` 与 `auction-base-backup.*`；只启用基础备份 timer 而不启用上传 timer 时，基础备份只在本地暂存。
8. 以上全部通过后，才在停机门禁允许的窗口设置 `archive_mode = on`、`archive_timeout = 300` 并重启 PostgreSQL；不能用 `pg_ctl reload` 代替。

### 验证与失败处置

```bash
psql -d auction -c 'SELECT archived_count, failed_count, last_archived_wal, last_failed_wal FROM pg_stat_archiver'
systemctl list-timers auction-wal-upload.timer auction-base-backup.timer
systemctl status auction-wal-upload.service auction-base-backup.service
df -h /var/lib/auction/wal-staging
```

`failed_count` 持续增长表示 `archive_command` 失败（zstd/age 缺失、权限、目录不可写），此时 `pg_wal` 会增长；暂存长期非空表示上传器失败或未启用。R2 核对对象数量、最新段名与每个 base 目录的 `completed.json`；本机基础备份应稳定 3 组、每组密文 + manifest + `.upload.json`。

| 现象 | 处置 |
| --- | --- |
| `failed_count` 增长、`zstd 压缩失败`/`age 加密失败` | 以 postgres 手工执行 `wal archive`，核对 PATH、公钥与目录权限；修复后 PostgreSQL 自动重试，不要手工删 `pg_wal` |
| manifest 存在但密文缺失 | 保留证据人工核对；确认密文不存在且段未被确认时，删除残留 manifest 让重试重新归档 |
| 同名段不同源内容 | 失败关闭不覆盖；核对段名复用或目录复制，保留全部证据 |
| `offsite_object_conflict` | 远端同名对象长度/摘要不同：保留本地密文，按[异机异常处置](#异机异常处置)核对，不覆盖或改名 |
| `pg_basebackup 失败` | 查 journal 的 stderr：连接、`REPLICATION`、额外表空间或 WAL 已回收；修复后手工重跑 |
| 基础备份 manifest 存在但密文缺失 | 失败关闭；确认密文不存在后删除残留 manifest 或整组，重新生成，不能补写 manifest |
| 密文 SHA-256 与 manifest 不一致 | 上传前失败关闭；该暂存项损坏，保留证据后由用户决定删除该组并重建 |
| `base.failed` 长期非空 | 按备份 ID 核对 R2 前缀、bucket lock 与令牌；本地副本保留，修复后自动对账 |
| 怀疑远端基础备份被删改 | 删除对应 `.upload.json` 后下一次运行会 HEAD + 完整回读；对象缺失时重新上传。远端只由 bucket 生命周期淘汰 |
| 暂存占满磁盘 | 先恢复上传；确需腾空间时按授权处理已上传且校验通过的对象 |
| 停止归档 | 先停 upload timer，再在停机窗口把 `archive_mode` 改回 `off` 并重启；未归档段留在 `pg_wal`，重启后继续尝试 |

### 时间点恢复

完整流程需要 R2 的一份基础备份与截至目标时刻的全部 WAL。本仓库尚未在隔离 PG18 上完整演练该流程；演练前恢复点仍以[定时备份](#定时备份)的日/月逻辑备份为主。

1. 在 `<prefix>/base/<备份 ID>/` 选备份，下载并核对 `completed.json`（`auction-base-upload.v1`、备份 ID、两份对象名/长度/SHA-256），再下载密文、manifest 与 `<prefix>/wal/` 中目标时刻之前且晚于备份起点的全部段；只有密文而没有完成记录时不能作为恢复点。
2. 在离线工作站用 age 私钥解密到 0700 目录，用 `wc -c` 与 `sha256sum` 核对记录与 manifest，再 `zstd -d` 还原 `base.tar` 与段文件名；不得在恢复机留下私钥或明文 WAL。
3. 展开 `base.tar` 为隔离实例数据目录（含备份期间 WAL），设置指向解密 WAL 的 `restore_command`、`recovery.signal` 与 `recovery_target_time`，启动并核对停止点。`-X fetch` 只含备份期间 WAL，之后仍依赖归档段。
4. 核对 schema/迁移版本、关键表与只读事实，按[外部历史核对](#6-外部历史核对与启动)排除备份后的邮件、附件与论坛写入；记录备份 ID、段范围、目标时刻、解密结果与耗时。一次完整演练后才计入可恢复性证据。

## 异机副本前置准备

以下步骤由用户执行。CLI 不生成或保存 age 私钥，不配置 Cloudflare 控制台，也不把托管材料装进数据库备份。

### 1. 离线生成与托管密钥

1. 在受信任的离线工作站安装 age，并在 0700 私有目录中执行下列命令。把私钥导入密码管理器，再保存一份加密离线介质副本；测试两处均能独立取回后，按本地安全流程移除工作副本。服务器只接收 `recipient.txt` 中的公钥，不接收 identity 文件或私钥口令。

   ```bash
   umask 077
   age-keygen -o backup-identity.txt
   age-keygen -y backup-identity.txt > recipient.txt
   ```

2. 与 age 私钥一起托管三组数据密钥：`equipment_data_key_id/equipment_data_key`、`user_login_data_key_id/user_login_data_key`、`credential_data_key_id/credential_data_key`。保存**标识、材料、用途、适用时间和历史版本**；三组互不复用，轮换后仍保留能解开保留期内旧备份的版本。数据库只保存密文，没有这些材料就不能恢复相应列。
3. 取回时先按备份日期与信封 key_id 选择历史版本，交叉核对密码管理器和离线副本中的清单；在隔离恢复机创建 0600 敏感 TOML，填入所需三组材料。age 私钥只在离线解密工作站使用，随后仅传递解密后的 archive/manifest 到隔离 PG18 主机。日志和演练回执只记录 key_id、取回是否成功及条数，不保存材料或解密明文。
4. 服务器通过受信任的软件包渠道安装主机 `age`，确认固定服务账号在 systemd PATH 中可执行 `age --version`。该命令会在每次启用异机的 scheduled 中、pg_dump 之前预检。实际加密使用公钥参数、关闭 stdin、不读取口令，不引入 Rust age crate。

### 2. 用户在 Cloudflare 控制台配置 R2

1. 创建专用 bucket，记录 S3 API 的 HTTPS 根 endpoint。统一选择普通配置的 `<prefix>`（例如 `auction`）；下面规则使用真实前缀，不使用占位文本。
2. 按下表为对象键前缀配置按对象年龄淘汰的生命周期和 bucket lock；启用备份之前核验规则已生效。`<prefix>` 为空时键直接为 `daily/`、`monthly/` 等，非空时为 `<prefix>/daily/`。锁定期不长于淘汰期，按决策使用相同天数。`<prefix>/wal/` 与 `<prefix>/base/` 由 WAL 上传器产生（见[WAL 归档与时间点恢复](#wal-归档与时间点恢复)），`base/` 下每个备份一个目录（`<prefix>/base/<备份 ID>/`）；默认 `archive_mode = off`，启用须按该节顺序执行。

   | 前缀 | 生命周期淘汰 | bucket lock |
   | --- | --- | --- |
   | `<prefix>/daily/` | 30 天 | 30 天 |
   | `<prefix>/monthly/` | 365 天 | 365 天 |
   | `<prefix>/wal/` | 21 天 | 21 天 |
   | `<prefix>/base/` | 21 天 | 21 天 |

3. 配置未完成分片的自动回收规则，例如 7 天，防止强杀遗留的 multipart 占用长期积累。这只回收未完成会话，不改变完整对象的锁定规则。
4. 创建**仅限该 bucket 的对象读写令牌**，服务器不使用账户级管理权限。R2 令牌档位无法单独排除删除，防删除/覆盖必须依靠 bucket lock。将 access key ID、secret 只写入 Worker 引用的 0600 敏感 TOML；令牌不进命令参数、环境变量、URL、回执或 shell 历史。下载演练可在受信任工作站另用限定 bucket 的只读令牌。
5. 核验清单：四个前缀的年龄规则与锁定期正确；令牌只覆盖该 bucket；在用户授权的专用核验对象上验证锁定期内删除/覆盖被拒绝，保留非秘密的状态码证据；R2 可 HEAD/GET/LIST；服务器公钥与离线托管私钥配对。不要用生产恢复点做删除核验。
6. 填写 `common.toml [backup.offsite]` 的公钥、endpoint、bucket、prefix 后设 `enabled = true`，以服务账号执行 `config check --strict`。这是纯本地检查，不能证明 bucket lock 或网络可用。下一次 timer 自动生效；首次上线需用户观察 daily 与 monthly 完成记录及运行回执。

**bucket lock 是启用异机副本的前置条件。** 单次 PUT 使用原生 create-only；分片开始前先 HEAD，已有对象只回读核对。object_store 的公开分片完成接口本身没有条件写入，工具不拦截 SDK 请求或改写签名头。分片不可变性依靠 bucket lock + 唯一 run_id 命名 + 重用完全相同的密文 + 上传后 HEAD/完整 SHA-256 校验共同保证；不能仅用 HEAD 排除并发覆盖。

### 3. 运行、重试与轮换

日期取明文 manifest 的 UTC `created_at`，日副本路径为 `<prefix>/daily/YYYY/MM/DD/<run_id>/`，月副本为 `monthly/YYYY/MM/<run_id>/`。同一密文依次上传 archive、manifest，最后写 `completed.json`（`auction-backup-upload.v1`）；只有完整校验通过且有完成记录才算恢复点。每个 UTC 备份月第一份完成副本另存月前缀；本机月标记缺失时遍历 LIST 的全部完成记录，解析/字段无效、缺件或长度/摘要不符的记录逐条跳过，journal 只输出固定原因码 monthly_record_invalid；第一份完整有效记录恢复标记，全部无效则正常创建本次月副本。LIST/GET 传输或拒绝访问仍使该项目失败。固定一个执行账号、目的地及状态目录，避免多个独立调度器争抢月首判断。

先处理当天新备份的加密、上传、校验和 finalize，再按时间重试旧项，有限预算优先保护最新恢复点。坏任务、原回执不匹配或目的地变化的旧项跳过并告警，保留原文件和密文；单个项目失败或密文冲突只记录该项并继续，其余成功项目照常 finalize。全部处理完或预算耗尽后，任一项目失败都会使本次整体 failed，failure 取首个失败；跳过告警本身不改变 outcome。回执 failed 不表示本次 daily 一定失败，应同时核对 offsite.daily 和远端完成记录。任务位于 `<state-dir>/offsite/pending/`，封存密文位于 `<output-dir>/offsite-staging/<run_id>/`，全部为私有目录/文件；阶段检查点保存相同字节的摘要和已完成对象。409/412 或响应未知时读取原对象：先 HEAD 长度，再按最多 64 MiB 的区间顺序 get_range，累计实际长度和完整 SHA-256；两者一致为 already_present，不同为冲突，绝不覆盖、删除或另取名称。每段单独受 300 秒请求超时约束，全部区间仍共享本次总预算。daily 与所需 monthly 均完成后才删除密文暂存与待上传项，历史运行回执永不改写。

远端对象由 bucket 生命周期按年龄淘汰，工具从不删除。正常每天一份时约保留 30 份日、12 份月；漏跑、补跑及日历月份长度会影响瞬时数量，这不是精确份数上限。关闭异机功能只需 `enabled = false`，下次运行暂停上传和旧项重试，原任务与未取得副本资格的本地备份继续保留；停止全部定时任务则停用 timer。变更 prefix、endpoint、bucket 或公钥后，目的地哈希不符的旧项会保留并跳过告警，新目标正常创建新恢复点；原任务不能自动改投新目标或重新加密，需按下表人工处理。令牌可以在保持目标不变时轮换。

## 从 R2 取回并演练恢复

目标必须是隔离 PG18 实例，禁用全部外部邮件、论坛与调度，不连生产库；每类密文至少一条样本，空数据族记为未验证。

1. 在 daily/monthly 选有 `completed.json` 的恢复点，核对 `auction-backup-upload.v1`、run_id、日期、对象名/长度/SHA-256、manifest 摘要与 age 版本；对象名须在同一前缀。只有两份密文而无完成记录不能选。
2. 范围证据（启用时）核对 `scope.bundle.age` 与 `scope.json` 的长度与摘要。解密 manifest 后在离线工作站执行：

   ```bash
   auction-cli backup scope-extract --manifest <解密后的manifest> --database <原库名> \
     --identity <age 私钥> --output-dir <全新私有目录>
   ```

   解出的配置、诊断证据与部署回执用于重建主机；不在生产备份服务器保留 identity 或解出目录。
3. 下载记录指定的两个对象并保持文件名，目录 0700、文件 0600；用 `wc -c` 与 `sha256sum` 核对记录值，不能用 multipart ETag 代替整文件摘要；不符则停止该恢复点。
4. 用离线私钥解密 archive 与 manifest，`umask 077`、不覆盖已有证据；manifest 摘要必须与完成记录的 `plaintext_manifest_sha256` 一致，两次认证成功后再传递。
5. 以原库名执行 `backup verify`，准备空库，执行 `db preflight`、`restore --dry-run` 与正式 `restore --receipt <新的0600回执>`；不在恢复前 migrate。
6. 从托管清单取回对应历史版本的三组数据密钥，在隔离环境只读读取样本信封，用 `EquipmentKeyOpener::open`、`AdminLoginKeyOpener::open`、`AdminCredentialOpener::open` 核验 ID/kind/scope/key_id/AAD；只输出数据族、样本 ID、key_id、成功条数与失败分类。不能改信封、重新封装或重新注册来证明旧材料可恢复。
7. 核对 schema/迁移版本、关键表与只读事实，记录完成记录摘要、对象名、下载校验、restore 回执、解密条数与耗时。恢复到生产前仍须做[外部历史核对](#6-外部历史核对与启动)。

## 异机异常处置

| 情况 | 处置 |
| --- | --- |
| age 私钥遗失 | 从另一托管副本取回；两处均遗失则既有密文不能解开，公钥和 R2 令牌不能替代私钥。保护仍可用的本地明文并由用户决定新的密钥和备份策略 |
| 某组数据密钥遗失或版本不符 | 即使 age 解密/数据库恢复成功，相关列仍不可用；从历史托管版本取回，不覆盖或重置恢复库中的旧信封 |
| 密文长度或摘要冲突 | 只使该项目失败，后续项目继续；按 offsite.failed_items 中的 run_id、stage、固定错误码核对原暂存、任务与远端对象，确认目的地、公钥和 bucket lock。保留全部原证据，不重新加密、覆盖、删除或换名规避冲突 |
| 跳过的待上传项 | 根据 skipped_pending 的原因码核对原 JSON、原回执、文件身份及权限；无法确认 run_id 时人工列举私有 pending 目录，不把内容粘进日志。不得编辑任务身份或摘要骗过校验；证据可修复时恢复原文件条件，无法修复时由用户将原任务与密文整体隔离保留 |
| pending_destination_changed | 新目标已继续备份；由用户决定是否在原目标/公钥配置下另行处理保留任务，或整体隔离原任务与密文。不能把旧任务哈希改成新目标、换名重传或重新加密；原数据密钥和 age 私钥版本仍须托管 |
| 完成记录缺失或缺件 | 不视为成功恢复点；优先选择另一个完整点。原暂存和任务尚在时修复条件后由 scheduled 继续，不能手工捏造完成记录 |
| 上传结果未知、409 或 412 | 让工具 HEAD + 完整回读对账；一致视为 already_present，不一致失败。不得把无响应当作未上传并改名重传 |
| 月标记丢失 | 保留其他状态，下一次遍历 monthly 完成记录；坏记录跳过，首份完整有效记录恢复标记，全部无效则创建新月副本。按 monthly_record_invalid 告警人工查看原坏证据，不删除或伪造完成记录；存储访问错误仍需修复后重试 |
| 状态或暂存损坏、长期积压 | 保留所有原证据，查看固定错误码、pending 与原运行回执；不要凭新公钥重新生成同名密文。修复存储/网络或增加容量，必要时由用户隔离完整旧任务，关闭异机不会自动删积压 |

## 程序回退

`deploy` 在安装前把当前四个程序保留到 `<state-dir>/rollback/<deploy_receipt_id>/`（`record.json` 记录安装目录、版本与四个 SHA-256）。`auction-cli rollback --receipt-id <deploy_receipt_id>`（省略取最新）只换回该记录的程序，并：

- 要求安装目录仍是该部署回执的版本，不一致时拒绝；
- 读取数据库版本，要求 `previous_version → 当前版本` 的迁移连续、成功且登记为 `compatible`；任一 `breaking` 或缺登记即拒绝，只能恢复升级前备份并做外部历史核对；
- 先过停机时段与在途发送门禁，再按 Public → Admin → Worker 停止；安装前把当前程序保留为新的回滚记录，因此回退可再前进；
- 启动后核对三个程序摘要，成功追加 `action=rollback` 回执。`--dry-run` 只核对记录、目录、区间与门禁。

手工换回旧程序须显式设置 `[readiness] migration_compatibility_mode = "compatible"` 并保留能识别当前数据库的新版 CLI；默认 shadow 不放宽。

迁移只向前，不执行 down。令 `E` 为目标旧程序内嵌最高版本、`D` 为当前库最高版本，只核对跨越的 `(E,D]`：

| 区间事实 | 可用路径 |
| --- | --- |
| `D = E`，完整历史严格匹配 | 换回同 schema 的旧程序 |
| `D > E`，`E+1…D` 连续成功且逐版本登记 `compatible` | 目标程序含兼容登记并显式启用 compatible 后可换回 |
| 含 breaking、缺登记、未知、迁移失败或无法确认 | 拒绝；恢复升级前备份并做外部历史核对 |
| `D < E` | 目标程序要求未完成的迁移，不能放行 |

001–074 不回填且缺行即 breaking，075 自身也登记 breaking；兼容能力须先随版本 A 部署，之后 B 才能换回 A。登记表与判定由[docs/contracts/database.md#迁移兼容登记与就绪](../../docs/contracts/database.md#迁移兼容登记与就绪)定义。

保留数据库的回退步骤：确认目标制品、`E`、配置兼容性与可用备份，按[停机时段门禁](../../apps/auction-cli/README.md#停机时段门禁)停止全部角色；保留已迁移到 `D` 的新版 CLI 只换角色程序，用 `db status`/`db preflight` 确认 `schema_current=true` 且无待执行迁移；以只读事务逐版核对 `(E,D]` 的 success 与兼容等级（结果须恰好覆盖区间，不能只筛 compatible 或只看最高版本）：

```sql
BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL statement_timeout = '5s';
SET LOCAL lock_timeout = '1s';
SELECT migration.version, migration.success, compatibility.level
FROM public._sqlx_migrations AS migration
LEFT JOIN public.schema_migration_compatibility AS compatibility ON compatibility.version = migration.version
WHERE migration.version > 75
ORDER BY migration.version;
ROLLBACK;
```

换回前把三角色共用 common 的 `[readiness] migration_compatibility_mode` 设为 `"compatible"`（启动快照，不由 runtime 覆盖，修改需重启）；默认 shadow 在数据库超前时不会就绪。替换角色程序并保留新版 CLI 与数据库，分体用新版 CLI `service start`（已确认无待执行迁移），合并正常启动旧 supervisor。记录版本、E/D、区间、模式与启动结果；只有部署回同版程序后才恢复 shadow，旧程序面对超前数据库时恢复 strict/shadow 会按设计撤销就绪。

平时保持迁移 shadow。日志 target `auction::storage::migration_readiness` 只输出 class、E/D 与两个判定布尔值；`auction_readiness_migration_shadow_mismatch_total{class}` 只允许 `compatible_ahead` 预期差异，分支覆盖见 `auction_readiness_migration_checks_total{branch}`。无样本不能证明超前分支已验证；切换由用户审阅，不自动开启 compatible。兼容放宽不授权旧 CLI 迁移、备份、恢复或盘点未来 schema；需要恢复数据库时继续走完整恢复流程。

## 恢复流程

### 1. 停止服务

```bash
systemctl stop auction-stack.service          # 分体部署
systemctl stop auction-server.service         # 合并部署
```

确认没有其他客户端（报表、手工 psql）连接目标库并阻止重连。合并部署的写库命令须以与 `auction-server` 相同的服务账号执行，`--config-dir` 改为实际目录（示例 unit 为 `/var/lib/auction/config`）；本页其余示例使用分体部署目录。

### 2. 准备同名空库

目标库仍存在时先只读演练，再执行：

```bash
auction-cli --config-dir /etc/auction db rebuild --database auction \
  --maintenance-secrets /etc/auction/maintenance/postgres.toml \
  --owner-secrets /etc/auction/owner/postgres.toml \
  --mode empty-for-restore --backup-manifest /var/backups/auction/auction-<时间>.dump.manifest

# 确认计划后加 --execute --confirm 'REBUILD auction' --receipt /var/backups/auction/rebuild-<时间>.json
```

演练先校验 manifest、archive 摘要并要求备份 schema 不高于当前 CLI，再持锁检查维护库、PG18 可写主节点、目标无自定义 ACL/设置、UTF8、零活动会话、无预备事务与逻辑复制槽。执行时用 `DROP DATABASE … WITH (FORCE)` + `CREATE DATABASE … TEMPLATE template0` 以原 owner、编码、locale、tablespace 与连接上限重建，并写 `auction-postgres-rebuild.v1` 回执。

目标库不存在（例如新集群）时用 `db create --database auction --maintenance-secrets … --owner-secrets …`；新库 owner 是迁移 owner，服务账号不是 owner。**不要在恢复前 `db migrate`**：恢复目标必须没有任何 schema。

### 3. 预检与演练

```bash
auction-cli --config-dir /etc/auction db preflight --database auction \
  --output /var/backups/auction/preflight-<时间>.json
auction-cli --config-dir /etc/auction backup restore --database auction \
  --owner-secrets /etc/auction/owner/postgres.toml \
  --manifest /var/backups/auction/auction-<时间>.dump.manifest --dry-run
```

`restore_preflight_passed = true` 只表示目标空库、无连接、可写主节点，不代表备份可用；dry-run 同时校验 archive。两者都不能阻止外部客户端在检查后重连。

### 4. 恢复

```bash
auction-cli --config-dir /etc/auction backup restore --database auction \
  --owner-secrets /etc/auction/owner/postgres.toml \
  --manifest /var/backups/auction/auction-<时间>.dump.manifest \
  --receipt /var/backups/auction/restore-<时间>.json
```

恢复前再次校验 archive，用 `pg_restore --single-transaction --exit-on-error --no-owner --no-privileges` 执行，完成后核对 schema 身份与迁移版本等于备份版本，写 `auction-postgres-restore.v1` 回执。单事务只保证提交前失败可回滚；客户端报错、连接中断或回执缺失都不能证明目标仍为空——保持停机用 `db preflight` 核对，若已提交则不能直接重跑恢复。

备份 schema 不得高于当前 CLI；恢复后对象归迁移 owner（`ownership_restored=false`）。备份含 `000081` 时命令自动重放 `apply_auction_runtime_privileges()`（`runtime_privileges_applied=true`），否则服务账号没有对象权限；函数缺失或备份更旧时保持停机并以非零退出，先 `db migrate` 再启动，或手工 `db grants`。

### 5. 迁移与权限

```bash
auction-cli --config-dir /etc/auction db status
auction-cli --config-dir /etc/auction db grants --owner-secrets /etc/auction/owner/postgres.toml
auction-cli --config-dir /etc/auction db migrate --owner-secrets /etc/auction/owner/postgres.toml
```

复核：`has_schema_privilege('auction_runtime','public','CREATE')=f`、`has_table_privilege('auction_runtime','bids','UPDATE')=f`、`has_table_privilege('auction_runtime','bids','INSERT')=t`。合并部署以服务账号执行 `db migrate`，完成外部历史核对后再启动。

### 6. 外部历史核对与启动

数据库回到备份时刻，但外部世界没有：Bot 可能已寄信、收附件或发帖。再次启动 Worker 前必须先暂停全部外部访问，再用 `recovery external-reconcile` 做有界只读核对；工具只找证据，不自动重发、领取或判定业务完成。

1. 发布带到期时间的暂停（到期自动恢复并留审计）：

   ```bash
   auction-cli --config-dir /etc/auction switch set external_access_paused true \
     --expires-at <RFC3339> --command-id <稳定UUID> --expected-revision <switch list 中的版本> \
     --actor <操作者说明> --reason <非敏感原因>
   ```

   暂停时 `recovery external-reconcile` 才是唯一允许的远端读取；未暂停时 Worker control 返回 `409 not_paused`。结果未知时用同一命令 ID 与参数重试；不要用 `--no-expiry` 代替到期时间。

2. 只读演练并保存回执：

   ```bash
   auction-cli --config-dir /etc/auction recovery external-reconcile \
     --cutoff <恢复点 manifest 的创建时间> --receipt /secure/recovery-<时间>.json
   ```

   工具经 Worker control 读 Sent 与 Read Mail 摘要，只为列表投影匹配的候选 MID 读详情，并读取最多 20 个已发布论坛主题的新楼层。回执的 `sent_complete`/`read_mail_complete`/`errors` 表示扫描覆盖到恢复点；`status=partial` 或截断时不能把“未观察到”当作“未发送”。`--cutoff` 只用于 Sent 与恢复点时钟；Read Mail 始终扫描到末页并按恢复库身份核对。`local.frozen_expectations` 是仍有 fence 的投递数，其余计入 `restored_payload_expectations`（结论记为 `restored_payload`，不是精确证明）。

3. 核对 `deliveries.planned`、`deliveries.unresolved`、`read_mail.missing_locally` 与 `forum.missing_posts`，确认后隔离：

   ```bash
   auction-cli --config-dir /etc/auction recovery external-reconcile \
     --cutoff <同一恢复点> --receipt /secure/recovery-<时间>-applied.json \
     --apply --confirm 'APPLY EXTERNAL RECONCILIATION' --actor ops --scan-id <第一次回执的 scan_id>
   ```

   `--apply` 在单事务内重新锁定投递并核对 fence：唯一匹配与无法唯一归属的投递都改为 `manual_reconcile`，分别追加 `delivery_external_quarantines` 与审计；不重发、不领取、不改写资金/结算。同 `scan_id` 重放幂等。

4. 工具不能自动完成的部分人工核对：备份后已收取的附件与 COD 收款（读恢复库、诊断证据与游戏内领取记录）；缺失的已读入站邮件（按 MID、发件人、标题与时间核对投稿/回执/换绑）；缺失论坛楼层（按 `thread_id`/`post_id` 与 `forum recovery`、`pm list` 核对竞价、notify 与私信）。任何未知 Send 保持 `manual_reconcile`，只有原 Sent 详情精确证明或 `settlement confirm-sent` 才能收口，绝不自动重发。
5. 确定恢复后先解除暂停（或等待到期），再启动对应 unit；通过 Admin“投递队列”“结算报表”复核，核对完成后才恢复之前停用的外部功能。

## 演练

决策 31 要求每月一次恢复演练。每次演练成功后，把 `backup restore` 写出的 `auction-postgres-restore.v1` 回执（或 `conclusion.result` 为 `passed` 的迁移演练回执）复制到主机的演练回执目录（模板为 `/var/lib/auction/restore-drills`，0700，归运行账号）；`auction-cli alert report --restore-drill-dir` 据此判断，超过 35 天没有成功演练时产生摘要级 `restore_drill_overdue`。回执只能复制原件，不要手写或修改。

定期在**隔离的 PostgreSQL 实例**上演练完整恢复。使用同名数据库（manifest 绑定库名摘要，不能恢复到其他库名）。一次性演练库可用 `db rebuild --ephemeral` 替代备份前提做 `fresh-schema` 重建，实际执行仍须提供 `--execute --confirm 'EPHEMERAL <库名>' --receipt <新回执>`；它不会生成恢复所需的空库，`empty-for-restore` 模式必须提供 `--backup-manifest` 且不接受 `--ephemeral`。

一次完整月度演练的最小命令序列（隔离实例，示例路径）：

```bash
# 1) 选取并校验恢复点（隔离配置指向 55432，不连生产）
auction-cli backup verify --database auction --manifest /srv/drill/source.dump.manifest
# 2) 准备空库 → 预检 → dry-run → 恢复
auction-cli db rebuild --database auction --maintenance-secrets … --owner-secrets … \
  --mode empty-for-restore --backup-manifest /srv/drill/source.dump.manifest \
  --execute --confirm 'REBUILD auction' --receipt /srv/drill/rebuild.json --no-systemd
auction-cli db preflight --database auction --output /srv/drill/empty-preflight.json
auction-cli backup restore --database auction --owner-secrets … \
  --manifest /srv/drill/source.dump.manifest --receipt /srv/drill/restore.json --no-systemd
# 3) 只读核对后把回执原件复制到演练目录（含 migrate/db status 结果）
install -d -o auction -g auction -m 0700 /var/lib/auction/restore-drills
install -o auction -g auction -m 0600 /srv/drill/restore.json \
  /var/lib/auction/restore-drills/restore-$(date -u +%Y%m%dT%H%M%SZ).json
auction-cli --config-dir /etc/auction alert report --backup-output-dir /var/backups/auction/scheduled \
  --backup-state-dir /var/lib/auction/backup --restore-drill-dir /var/lib/auction/restore-drills
```

演练不通过（恢复回执 `schema_version` 不符、`runtime_privileges_applied=false`、只读核对发现差异）时不要复制回执；保留失败证据并按[回执状态判读](#回执状态判读)处理。

含迁移版本部署前，还须按[ops/deploy/MIGRATION-DRILL.md](MIGRATION-DRILL.md)用最新备份和候选程序完成升级、readiness 并留存[ops/deploy/migration-drill-receipt.template.json](migration-drill-receipt.template.json)。恢复演练与新空库安装不能替代旧备份到候选 schema 的升级演练；该回执中的完整停机耗时用于设置 CLI 时段保护窗口。

## 全新空库

需要一个带当前 schema 的全新库（例如测试环境）时：

```bash
auction-cli db rebuild --database auction --maintenance-secrets … --owner-secrets … \
  --mode fresh-schema --backup-manifest <现有库的备份 manifest> \
  --execute --confirm 'REBUILD auction' --receipt <回执>
```

或对新建的空库直接执行 `auction-cli db migrate --owner-secrets …`（空库执行全部迁移时 000081 会自动授予运行账号权限）。

## 回执状态判读

本页拥有恢复/重建/演练的操作步骤与回执判读；子命令参数与退出码由[CLI README](../../apps/auction-cli/README.md)拥有。

`auction-postgres-restore.v1`：`restored_at`（UTC，演练目录只认此格式）、`database`（须等于 manifest 原库名）、`archive_file_name`/`archive_sha256`/`manifest_sha256`、`schema_version`（须等于 manifest 版本）、`single_transaction`（恒 true）、`ownership_restored`/`privileges_restored`/`runtime_privileges_applied`（恒 `false`/`true`/`true`；后两者 false 说明服务账号可能缺少对象权限）、`scope_evidence_present`（只表示带 sidecar，不表示已核对）。

`auction-postgres-rebuild.v1`：`outcome`（`completed`/`failed`）；`rebuild`（DROP/CREATE 成功时的原/新 OID、owner 与建库参数）；`failure.stage`（`drop_database`、`create_database`、`verify_recreated_database` 或 `migrate`）；`failure.database_dropped`/`database_recreated`（`null` 表示无法确认）。

失败关闭：回执缺失、`outcome != completed`、`schema_version` 与 manifest 不符、`runtime_privileges_applied=false` 且版本 ≥ 000081、或响应丢失为 `null` 时，保持三角色停止并人工核对 catalog 与备份；不得启动 Worker、重跑破坏性命令或把“未观察到”当未执行。用 `db preflight` 与 `psql` 核对库是否存在、是否为空、OID 是否变化；进程在 DROP 后被强杀可能来不及写回执，此时以 catalog 与备份为准。

## 数据保留

`auction-cli db inventory` 按[ops/deploy/retention-policy.v2.json](retention-policy.v2.json)统计各数据族的规模，只读、不授权任何归档或删除。`auction-cli db retention-dry-run` 按政策期限统计运行态族的候选行与加密诊断文件候选数，只执行 SELECT 并输出带 `run_id` 的私有回执；`mutation_executed`/`deletion_authorized` 恒为 false。审计、资金与业务事实永久保留。

实际清理只经 `auction-cli db retention-purge`：它绑定 15 分钟内、当前政策身份的演练回执与逐数据族 `approval_token`，要求 `--execute`、`PURGE <库名>` 逐字确认和不可覆盖回执；只删除政策 v2 的运行态行与普通过期诊断文件，有效/永久会话、未解决失败、写入未知诊断与审计/资金/业务事实永不删除。每批独立事务，批准计数是硬上限，漂移候选留待重新演练；无论成功或失败都会追加 `auction.retention.purge` 审计。执行前仍须由用户批准候选计数，且建议先在隔离克隆实例演练。
