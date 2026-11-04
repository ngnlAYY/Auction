# 备份与恢复手册

[文档导航](../../docs/README.md) · [运维手册](../README.md) · [auction-cli](../../apps/auction-cli/README.md) · [数据库契约](../../docs/contracts/database.md)

本手册描述 PostgreSQL 数据的备份、校验、恢复与破坏性重建。命令参数的完整说明见 [auction-cli](../../apps/auction-cli/README.md#备份与恢复)。所有写数据库的步骤都要求先停止三个角色（停机门禁），并在维护窗口内执行。

## 原则

- **备份 = archive + manifest。** `backup create` 生成 `<名称>.dump`（`pg_dump` 自定义格式）与 `<名称>.dump.manifest`（格式 `auction-postgres-backup.v1`：archive SHA-256、库名摘要、schema 版本、创建时间）。两者必须一起保存、一起复制到异地。manifest 没有数字签名，存储的访问控制与不可变性由部署方负责。
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

发布包的分体与合并形态均包含 `systemd/auction-backup.service.example` 与 `systemd/auction-backup.timer.example`（仓库源为 `ops/deploy/systemd/` 下同名文件），以及 WAL 上传器的 `systemd/auction-wal-upload.service.example` 与 `systemd/auction-wal-upload.timer.example`。启用由用户执行；启用 timer 同时授权按下述规则清理这个专用目录内的旧定时备份。备份范围仍只有数据库；可选异机副本将 archive 与 manifest 分别加密上传 R2。runtime 配置、诊断证据和每周物理基础备份由后续工作项交付；WAL 归档与上传见[WAL 归档与时间点恢复](#wal-归档与时间点恢复)。

timer 的 `ExecStart` 调用 `auction-cli backup scheduled`；线上旧 CLI `f99a9d4e` 早于该子命令，**必须先安装包含该命令的候选 CLI，再启用 timer**。R2、`age` 与监控服务可以在部署前准备，但旧程序上的 timer/出口验收不成立。完整顺序见[运维手册的 M1 候选上线顺序](../README.md#m1-候选上线顺序备份与指标前置)。

先确认发布包中的 `auction-cli` 带 postgres feature，运行账号能读取配置，PATH 中的 `pg_dump/pg_restore` 为 PG18 客户端；启用异机副本还须按下节安装主机 `age` 并完成离线托管及 bucket lock 设置。根据实际库名和目录编辑 service。以下安装命令在解压后的发布包根目录以 root 执行：

```bash
install -d -o auction -g auction -m 0700 /var/backups/auction/scheduled /var/lib/auction/backup
install -m 0644 systemd/auction-backup.service.example /etc/systemd/system/auction-backup.service
install -m 0644 systemd/auction-backup.timer.example /etc/systemd/system/auction-backup.timer
sudo -u auction /opt/auction/bin/auction-cli --config-dir /etc/auction config check --strict
systemctl daemon-reload
systemctl enable --now auction-backup.timer
```

合并部署将 service 的 `--config-dir` 改为 `/var/lib/auction/config`。两个目录必须是不重叠的私有真实目录，运行账号固定；状态目录保存历史回执与运行记录，不要因更新程序而删除。普通参数在 `common.toml [backup]`，见[配置参考](../config/README.md#定时备份策略)，修改后下一次 CLI 调用即生效，无需重启角色。停用回退为 `systemctl disable --now auction-backup.timer`；已经在执行的 oneshot 不因停用 timer 自动停止，若需中止由用户另行停止 service。停用不影响 Public/Admin/Worker，手动 create/verify/restore 继续可用。

默认排期由 `crates/auction-config/src/config/settings/auction/policy.rs` 的 `AuctionScheduleConfig::default` 与 `AuctionEconomicsConfig::default` 定义：UTC 周五 00:00 开拍，周六 12:00 名义结束，Final 提前 900 秒；`AuctionEarlyBirdConfig::default` 的早鸟及赛季前场次均在 18:00 名义结束。结算只在 Final 中所有 lot 的有效截止过去后启动（见[结算与履约](../../docs/guide/settlement.md#1-结算计算)）。timer 选每日 UTC 03:15，随机延迟最多 15 分钟，CLI 最多两小时，避开这两组默认 Final/结算高峰。软截止延长、自定义排期和 Persistent 停机补跑可能改变实际重叠情况，部署者需按实际时段调整 timer。`Persistent=true` 只补一次错过的唤醒，不补造停机期间每天的备份。

任务先取得状态目录独占锁，再收口上次运行；取锁失败只写 stderr 并退出 3，不写回执。持锁后建立本次运行记录，只加载 Worker 及其引用文件并校验备份策略，Public/Admin 的无关配置不会阻断备份；联合 `config check --strict` 作为运维预检保留。总预算从命令启动计算（默认 7200 秒），创建、加密、旧项重试和日/月上传共用此期限，到期写 failed 并退出 3；活动项目记 timeout，顶层 failure 保留首个失败（没有更早错误时为 timeout）；service 的 130 分钟预算另给终态同步和打卡余量。磁盘在 create 前后检查，异机启用时还预留待写密文空间；暂存目录必须与输出同文件系统，已有暂存自然计入占用。按非特权可用空间判断，扣除预留后字节和比例都必须达标；不足记 `disk_low`，不通过删除仍应保留的恢复点腾空间。create、verify 或已启用的异机阶段失败都不进入本次本地清理。失败以退出码 3、终态回执和 journal 反映。

本地保留默认最近 7 份：保留本次已校验的新备份及最近 6 份合格旧备份。清理只认同一输出目录、库名摘要、scheduled 命名和 completed 回执，检查已校验标志、设备号/inode/大小及 owner-only 单链接普通文件；该次启用异机时还要求已校验的 daily 副本。旧 failed/interrupted 上传项重试成功后追加绑定原 manifest 的 `offsite/completed/<run_id>.json` 完成事实，供清理使用，不改写原失败/中断回执。旧 v1 未启用异机的 completed 回执保持原资格。删除前不重新 verify 或重读 archive；本地校验前失败的运行回执静默忽略，不反复记副本告警；已经本地校验但 daily 资格不足才记 replica_not_verified。实际异常、手动、不完整或缺少完成证据的目录项仍保留并按清理规则跳过。`offsite-staging/` 不进入保留枚举或跳过噪音，历史回执不清理。异机长期失败使本地备份和密文累积，最终触发 `disk_low`，属于失败关闭；用户应修复上传或增加容量，不自动扩大删除范围。

查看 timer、service 与日志：

```bash
systemctl list-timers auction-backup.timer
systemctl status auction-backup.service
journalctl -u auction-backup.service
```

每次运行的回执为 `/var/lib/auction/backup/receipts/<run_id>.json`，格式 `auction-backup-run.v1`，字段及错误码见[CLI 回执契约](../../apps/auction-cli/README.md#定时备份)。回执为 0600、临时文件同步后无覆盖原子改名发布，`completed/failed/interrupted` 都是不可变终态；运行中状态为 `<state-dir>/running.json`，阶段更新使用原子替换。状态目录锁排除并发进程，无记录租约。强杀或断电当次可能来不及写终态，当次只依赖 systemd 失败状态和 journal；下一次 scheduled 取得同一目录锁后，对有效残留记录追加 interrupted。终态已发布且身份、已知 archive 证据一致时沿用原终态，不改成中断。无效记录或与同名终态冲突的记录，以 0600、`RENAME_NOREPLACE` 原样改名保留为 `<state-dir>/running.invalid-<UTC时间戳>-<本次run_id>.json`；本次回执增加 `recovery_warnings`，journal 告警只显示原因码及保留文件名，随后照常执行备份。保留文件需人工查看，历史终态不修改。只有存储读写或保留改名失败才报告 `recovery_failed` 并停止。不要直接把残留 archive 当作已校验备份。

| 回执错误码/现象 | 排查动作 |
| --- | --- |
| `config_error` | 以 auction 账号运行 `config check --strict`，检查配置文件、权限及备份范围参数；不要把敏感 TOML 或 URL 粘进日志 |
| 状态目录取锁失败（无回执） / `output_lock_busy_or_invalid` | 查看是否已有 scheduled 进程使用该状态或输出目录；锁通过描述符释放，不以删除锁文件代替核对 |
| `disk_low` / `disk_check_failed` | 检查该输出目录所在文件系统及回执水位；保留恢复点，按授权范围处理其他占用 |
| `create_failed` / `verify_failed` | 查看 service journal、PG18 客户端和数据库连接条件；修复后发起新运行，保留旧失败记录 |
| `offsite_preflight_failed` | 运行账号的 PATH 中检查 `age --version` 和可执行权限；此错误发生在 pg_dump 前 |
| `offsite_encryption_failed` / `offsite_local_read_failed` | 核对公钥、暂存权限、文件身份与磁盘；不丢弃或重新加密已经封存的密文 |
| `offsite_access_denied` / `offsite_transport_failed` / `offsite_daily_failed` | 检查限定 bucket 的令牌、endpoint、网络与 bucket lock；修复后下一次 scheduled 自动用原密文重试 |
| `offsite_retry_failed` / `offsite_state_failed` | 核对状态目录及 offsite.failed_items；逐项失败不会阻断其他项目，状态目录整体不可枚举或运行记录不可写则停止推进 |
| `offsite.skipped_pending` 非空 | 按固定原因码和可选 run_id 核对原 pending、原回执与密文；格式/身份不符和目的地变化均保留告警并跳过，不阻断本次新副本，人工处理步骤见下文 |
| `offsite_object_conflict` / `offsite_monthly_failed` / `offsite_remote_record_invalid` | 按下文异常处置核对完成记录和两份密文；禁止覆盖、删除或换名绕过冲突 |
| `retention_failed` / 跳过数量上升 | 核对回执、文件权限、硬链接和文件身份变化；工具不自动删除不明文件 |
| `timeout` | 先核对备份大小与资源占用，再在既定范围内调整内部预算；systemd 上限应继续更长 |
| `recovery_warnings` 含 `running_record_invalid` / `terminal_receipt_conflict` | 备份继续执行；按 journal 中的保留文件名人工检查 `running.invalid-*.json` 与相关历史终态，不将文件内容粘进日志；保留文件不会自动删除 |
| `recovery_failed` | 检查状态/回执目录的读写、权限、磁盘以及保留改名是否失败；修复存储条件后重试 |
| `receipt_failed` / 只有 running 记录 | 检查状态目录的权限和磁盘，下一次启动按已有终态或 interrupted 收口 |

外部打卡服务当前暂缓启用，Worker 敏感 TOML 两个打卡地址保持省略/空值，此时不发网络请求。以后启用时，CLI 先发布终态回执，再向成功或失败地址发一次 HTTPS GET，独立客户端不带 Cookie、不跟随重定向、不读代理环境；连接三秒、总预算五秒，不重试。回执只保存 `heartbeat_configured` 标志；`sent/failed/not_configured` 与 HTTP 状态码只经 stderr 进入 systemd journal，不写 URL 或独立打卡回执，打卡失败不会改写备份结果或退出码。在用户启用外部服务前，失败只通过回执、systemd 状态和日志查看。

## WAL 归档与时间点恢复

目标（决策 18、31、41、49）：RPO ≤ 5 分钟（`archive_timeout = 300`）、RTO ≤ 2 小时，可恢复到过去 **14 天**内任一时刻；WAL 与基础备份都保留 **21 天**，同时保有 3 份基础备份。两者使用 R2 的独立前缀 `<prefix>/wal/` 与 `<prefix>/base/`，生命周期与 bucket lock 均为 21 天，与日/月备份前缀互不影响。

本仓库当前交付的切片：`archive_mode`/`archive_timeout` 模板、`archive_command` 调用的 `auction-cli wal archive`（zstd 压缩 + age 公钥加密 + fsync 暂存）、独立上传器 `auction-cli wal upload`（R2 `wal/` 前缀、校验后删除本地副本）。**每周 `pg_basebackup` 物理基础备份与 `base/` 前缀尚未交付**，所以时间点恢复目前不能闭环；基础备份落地前，恢复点仍以[定时备份](#定时备份)的日/月逻辑备份为准。归档、上传与暂存目录大小的指标和外部打卡也尚未实现，现阶段只能靠 `pg_stat_archiver`、`systemctl` 状态与磁盘占用观察。

### 组成与数据流

1. PostgreSQL archiver 在段写满或 `archive_timeout` 到期后调用：

   ```conf
   archive_command = '/opt/auction/bin/auction-cli wal archive --staging-dir /var/lib/auction/wal-staging --recipient age1<收件人公钥> %p %f'
   ```

   命令以 **postgres 账号**执行，不读敏感 TOML；收件人公钥写在参数里（公钥不是秘密，私钥仍按[离线托管](#1-离线生成与托管密钥)保存）。它把 `zstd` 压缩流经 `age --encrypt --recipient` 写入 `<段名>.zst.age`，`fsync` 文件与目录后发布 `<段名>.zst.age.manifest.json`（`auction-wal-archive.v1`：段名、源字节数与 mtime、密文长度、归档时刻）。任一步失败返回非零，由 PostgreSQL 重试同一段；同一段已有 manifest 且源文件字节数与 mtime 一致时直接成功退出，不重复加密。源内容不同或密文长度与 manifest 不符时失败关闭，不覆盖既有证据。
2. `auction-wal-upload.timer` 每 5 分钟以 **auction 账号**运行 `auction-cli wal upload --staging-dir /var/lib/auction/wal-staging --budget-seconds 300`。它按段名顺序上传到 `<prefix>/wal/<段名>.zst.age`；远端已有相同长度与完整 SHA-256 的对象视为成功（bucket lock 下重传会返回 409），校验通过后删除本地密文与 manifest。
3. 上传失败、冲突或预算耗尽都保留本地文件并以非零退出；stdout 的 JSON 报告逐项给出 `uploaded`、`already_present`、`failed`、`invalid`、`incomplete` 与 `remaining`，systemd 记录失败状态。`incomplete` 是归档进程尚未发布完成的项（含只剩 manifest 的残留），不是上传失败。

### 启用顺序

1. 安装包含 `wal archive`/`wal upload` 的 `auction-cli`（旧 CLI 没有该子命令，先装候选再改 PostgreSQL）。
2. 在归档账号（postgres）的 PATH 中安装 `zstd` 与 `age`，确认 `zstd --version`、`age --version` 可执行；服务器只放公钥。同时确认 postgres 账号可执行 `/opt/auction/bin/auction-cli`（目录可遍历、二进制可执行），否则 `archive_command` 直接返回 127。
3. 建立暂存目录并确认权限：`postgres:auction`、`2770`（setgid，使密文继承 `auction` 组）。可用 `ops/deploy/tmpfiles/hv-auction.conf.example` 中对应的 `d /var/lib/auction/wal-staging 2770 postgres auction - -` 行，或手工 `install -d -o postgres -g auction -m 2770 /var/lib/auction/wal-staging`。
4. 以 auction 账号执行 `auction-cli --config-dir /etc/auction config check --strict`，确认 `[backup.offsite]` 已启用且 R2 令牌可写 `<prefix>/wal/`。
5. 手工验证一次归档与上传（不改 `archive_mode`）：

   ```bash
   sudo -u postgres /opt/auction/bin/auction-cli wal archive \
     --staging-dir /var/lib/auction/wal-staging --recipient age1<收件人公钥> \
     /path/to/pg_wal/000000010000000000000001 000000010000000000000001
   sudo -u auction /opt/auction/bin/auction-cli --config-dir /etc/auction wal upload \
     --staging-dir /var/lib/auction/wal-staging --budget-seconds 300
   ```

   重跑同一 `wal archive` 必须输出 `already_archived` 并退出 0；重跑上传必须报 `already_present` 或空集合。
6. 安装并启用 `auction-wal-upload.service/timer`（发布包 `systemd/` 目录），确认一次成功运行。
7. **只有以上步骤都通过后**，才在 `postgresql.conf` 打开归档并在 W7.5 允许的窗口重启：

   ```conf
   archive_mode = on
   archive_timeout = 300
   ```

   开启 `archive_mode` 需要重启 PostgreSQL，受[停机时段门禁](../../apps/auction-cli/README.md#停机时段门禁)约束；不能用 `pg_ctl reload` 代替，也不要在 Final 或临近截止的窗口执行。

### 验证

```bash
psql -d auction -c 'SELECT archived_count, failed_count, last_archived_wal, last_failed_wal FROM pg_stat_archiver'
systemctl list-timers auction-wal-upload.timer
systemctl status auction-wal-upload.service
journalctl -u auction-wal-upload.service
df -h /var/lib/auction/wal-staging
```

`failed_count` 持续增长表示 `archive_command` 失败（zstd/age 缺失、权限、暂存目录不可写），此时 `pg_wal` 会增长；暂存目录长期非空表示上传器失败或未启用 timer。R2 侧核对 `<prefix>/wal/` 下对象数量与最新段名，不与日/月备份混用前缀。

### 失败处置

| 现象 | 处置 |
| --- | --- |
| `pg_stat_archiver.failed_count` 增长，journal 出现 `zstd 压缩失败` / `age 加密失败` | 以 postgres 账号手工执行一次 `wal archive`，核对 PATH、公钥与暂存目录权限；修复后 PostgreSQL 自动重试同一段，不需要手工删 `pg_wal` |
| `WAL manifest 存在但密文缺失` | 保留 manifest 与目录，人工核对是否有人删除了密文；确认密文确实不存在且该段尚未被确认时，删除残留 manifest 让下一次重试重新归档 |
| `同名 WAL 段已有不同源内容的归档` | 失败关闭，不覆盖。核对段名是否被另一实例复用、暂存目录是否被复制；处理前先保留全部证据 |
| 上传器 `offsite_object_conflict` | 远端同名对象长度或摘要不同：保留本地密文，按[异机异常处置](#异机异常处置)核对 bucket、prefix 与令牌，不覆盖、不改名 |
| 暂存目录占满磁盘 | 先确认上传器与 R2 可用并恢复上传，不要直接删除未上传密文；确需腾空间时按授权范围处理已上传且校验通过的对象 |
| 需要停止归档 | 先 `systemctl disable --now auction-wal-upload.timer`，再在 W7.5 窗口内把 `archive_mode` 改回 `off` 并重启；停机期间未归档的段留在 `pg_wal`，重启后 PostgreSQL 会继续尝试归档 |

### 时间点恢复（基础备份交付后）

完整流程需要 R2 中的一份基础备份与截至目标时刻的全部 WAL。基础备份交付前的替代做法是[从 R2 取回并演练恢复](#从-r2-取回并演练恢复)中的逻辑备份恢复，它只能恢复到备份时刻。

1. 在隔离 PG18 实例上，用离线 age 私钥解密 `base/` 中的基础备份与 `wal/` 中目标时刻之前的全部段，逐个用 `wc -c` 与 `sha256sum` 核对远端记录，再用 `zstd -d` 还原原始文件名。
2. 把基础备份展开为数据目录，写入 `postgresql.auto.conf` 的 `restore_command` 指向解密后的 WAL 目录，创建 `recovery.signal`，在 `recovery_target_time` 指定目标时刻，启动实例并核对停止点日志。
3. 核对 schema/迁移版本、关键表与只读业务事实，然后按[外部历史核对](#6-外部历史核对与启动)排除备份之后已经发生的邮件、附件与论坛写入；恢复库不能直接接生产外部功能。
4. 记录使用的段范围、目标时刻、解密与还原结果、恢复耗时与外部核对结论。不要在演练机上保留私钥或明文 WAL。


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
2. 按下表为对象键前缀配置按对象年龄淘汰的生命周期和 bucket lock；启用备份之前核验规则已生效。`<prefix>` 为空时键直接为 `daily/`、`monthly/` 等，非空时为 `<prefix>/daily/`。锁定期不长于淘汰期，按决策使用相同天数。`<prefix>/wal/` 由 W10.7 上传器产生（见[WAL 归档与时间点恢复](#wal-归档与时间点恢复)），`<prefix>/base/` 仍待每周基础备份交付；默认 `archive_mode = off`，启用须按该节顺序执行。

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

此流程需要用户另行执行，目标必须是**隔离 PG18 实例**，禁用全部外部邮件、论坛与调度，不连生产数据库或游戏服务。一次演练应使用真实备份中每类至少一条密文样本；空数据族记为未验证，不能当作解密成功。

1. 在 R2 的 daily/monthly 中选择有 `completed.json` 的恢复点。下载完成记录，检查 `format = auction-backup-upload.v1`、run_id、UTC 日期、两份对象名/长度/密文 SHA-256、明文 manifest SHA-256、age 版本和校验时间；对象名必须都在同一恢复点前缀内。只有两份密文而无完成记录时不能选择该点。
2. 在受信任工作站通过控制台或已授权的 S3 客户端下载**记录指定的两个对象**，保持 `<archive>.age` 与 `<archive>.manifest.age` 文件名。下载目录为 0700，文件为 0600。逐一用 `wc -c < 文件` 和 `sha256sum 文件` 核对记录的 bytes 与 sha256；不能使用 multipart ETag 代替整文件摘要。任一不符则保留证据并停止该恢复点。
3. 按上节从密码管理器或离线副本取回对应 age 私钥，在离线工作站解密到新文件，禁止覆盖已有恢复证据：

   ```bash
   umask 077
   age --decrypt --identity backup-identity.txt --output scheduled-<时间>-<run_id>.dump \
     scheduled-<时间>-<run_id>.dump.age
   age --decrypt --identity backup-identity.txt --output scheduled-<时间>-<run_id>.dump.manifest \
     scheduled-<时间>-<run_id>.dump.manifest.age
   sha256sum scheduled-<时间>-<run_id>.dump.manifest
   ```

   manifest 明文摘要必须与完成记录的 `plaintext_manifest_sha256` 一致；两次解密认证均成功后，再安全传递 archive/manifest。不要把 identity 文件放到生产备份服务器。
4. 使用与 manifest 绑定的**原库名**执行离线校验：`auction-cli backup verify --database <原库名> --manifest <解密后的manifest>`。隔离配置指向 PG18 独立端口/实例，三角色保持停止；准备空库，执行 `db preflight`、`backup restore --dry-run`，再执行 `backup restore --receipt <新的0600回执>`，具体门禁和参数见[恢复流程](#恢复流程)。不在恢复前 migrate，不用 fresh-schema 代替空库。
5. 从托管清单取回对应历史版本的三组数据密钥，在隔离环境只读读取恢复库中的样本信封，使用现有 owner 的解密入口核验：装备 `EquipmentKeyOpener::open`，用户登录 key `MoogleMailLoginKeyReader` 的 `AdminLoginKeyOpener::open`，外部 Cookie `CredentialSealer` 的 `AdminCredentialOpener::open`。保留原装备/用户/凭据 ID、kind、scope、key_id 和 AAD 绑定，验证认证标签与指纹；不能通过改信封、重新封装或重新注册来证明旧材料可恢复。由监督者授权的隔离核验程序调用这些入口，只输出数据族、样本 ID、key_id、成功条数和失败分类，不输出密钥、Cookie、登录 key 或装备明文；当前 CLI 没有通用“导出解密列”命令。
6. 核对恢复 schema/迁移版本、关键表和只读业务事实，记录完成记录自身摘要、R2 对象名、下载校验结果、restore 回执、三组材料的解密条数和演练耗时。恢复到生产前仍须执行[外部历史核对](#6-外部历史核对与启动)，不因一次隔离演练而自动放行外部重发。清理恢复工作副本按托管流程执行，长期保留非秘密演练回执。

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

迁移只向前，不执行 down、不删除 `_sqlx_migrations` 或兼容登记。令 `E` 为目标旧程序内嵌的最高迁移版本，`D` 为当前数据库最高版本，只核对本次回退跨越的 `(E,D]` 区间：

| 区间事实 | 可用路径 |
| --- | --- |
| `D = E`，完整历史严格匹配 | 按原流程换回同 schema 的旧程序 |
| `D > E`，已知前缀严格匹配，`E+1…D` 连续成功且逐版本登记 `compatible` | 目标程序已包含 W7.8a，并显式启用 compatible 后，可保留当前数据库换回程序 |
| 区间含 breaking、缺登记、登记未知、迁移失败或无法确认 | 拒绝直接换回；恢复升级前备份，并执行下述外部历史核对 |
| `D < E` | 目标程序要求尚未完成的迁移，不能通过兼容开关放行 |

001–074 不回填且缺行即 breaking，075 自身也登记 breaking。兼容能力须先随版本 A 部署，之后版本 B 才能换回 A；不能因此回到尚不含该逻辑的程序。登记表及精确判定由[数据库契约](../../docs/contracts/database.md#迁移兼容登记与就绪)定义。

保留数据库的回退步骤由运维在维护窗口受控执行：

1. 确认目标制品、其内嵌最高版本 E、配置兼容性与可用备份，按[停机时段门禁](../../apps/auction-cli/README.md#停机时段门禁)停止全部角色；合并部署停止 supervisor 并等待子进程退出。
2. **保留本次升级已迁移到 D 的新版 `auction-cli`，只换角色程序。** 用该 CLI 的 `db status` / `db preflight` 严格核对当前库，要求 `schema_current=true`、没有待执行迁移；工具必须认识当前数据库。不能换回旧 CLI 后假定其 `service restart` 可处理未来版本，也不能用包含额外待迁移版本的工具执行 start。合并部署换回已含 W7.8a 的旧 `auction-server`，另行保留新版 CLI。
3. 在取得查询授权后，以只读事务逐版核对 `(E,D]` 的成功记录与兼容登记，保存非秘密核对证据。以下示例的 75 必须替换成目标制品 E；结果须恰好覆盖 `E+1…D`，所有 success 为 true、level 为 compatible，不能只筛选 compatible 行或只检查最高版本。

```sql
BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;
SET LOCAL statement_timeout = '5s';
SET LOCAL lock_timeout = '1s';
SELECT migration.version, migration.success, compatibility.level
FROM public._sqlx_migrations AS migration
LEFT JOIN public.schema_migration_compatibility AS compatibility
  ON compatibility.version = migration.version
WHERE migration.version > 75
ORDER BY migration.version;
ROLLBACK;
```

4. **换回旧程序前，必须把三角色共用 common 的 `[readiness] migration_compatibility_mode` 显式设为 `"compatible"`。** 默认 shadow 的实际判定仍严格，数据库超前时不会就绪；不是首次部署能力后自动放宽。该字段为启动快照，不由 Admin runtime 覆盖，修改需重启。
5. 替换角色程序，保留新版 CLI 与当前数据库。分体部署使用该新版 CLI 的 `service start` 按序启动（上一步已确认没有待执行迁移）；合并部署正常启动旧 supervisor，由其注入同一策略作只读启动门禁。核对就绪与既有健康条件。Worker control 自身的运行状态不能替代数据库兼容证明。
6. 记录制品版本、E/D、区间登记、模式与启动结果。只有部署回与数据库同版的程序后，才将迁移模式恢复 shadow；在旧程序面对超前数据库时恢复 strict/shadow，会按设计撤销就绪。

平时保持迁移 shadow。影子日志 target 为 `auction::storage::migration_readiness`，只输出固定 class、E/D 和两种判定布尔值；指标 `auction_readiness_migration_shadow_mismatch_total{class}` 只允许 `compatible_ahead` 预期差异，分支覆盖见 `auction_readiness_migration_checks_total{branch}`。同版无差异、无样本均不能证明超前分支已经验证；观察与切换由用户审阅，不自动开启 compatible，不擅自制造线上样本。命名暂沿用 W0.14 风格，待 W0.15 统一。

兼容放宽不授权旧 CLI 迁移、备份、恢复或盘点未来 schema；这些能力继续要求其严格认识的完整历史。不提供自动 rollback 命令。只要需要恢复数据库，就继续使用以下完整恢复流程，尤其不能省略备份之后的外部邮件、附件和论坛历史核对。

## 恢复流程

### 1. 停止服务

```bash
systemctl stop auction-stack.service          # 分体部署
systemctl stop auction-server.service         # 合并部署
```

同时确认没有其他客户端（报表、手工 psql 会话）连接目标库，并阻止它们自动重连。合并部署的后续写数据库命令须以与 `auction-server` 相同的服务账号执行，并把 `--config-dir` 改为其实际目录（示例 unit 为 `/var/lib/auction/config`）；本页其余示例采用分体部署目录。

### 2. 准备同名空库

目标库仍存在时，先只读演练：

```bash
auction-cli --config-dir /etc/auction db rebuild --database auction \
  --maintenance-secrets /etc/auction/maintenance/postgres.toml \
  --mode empty-for-restore \
  --backup-manifest /var/backups/auction/auction-<时间>.dump.manifest
```

演练先校验备份的 manifest、archive 摘要与目录，并要求备份 schema 版本不高于当前 `auction-cli` 支持的版本；此版本门禁在任何 DROP/CREATE 前执行，离线 `backup verify` 通过本身不代表当前工具支持恢复该版本。随后持锁检查：维护库为 `postgres`、PostgreSQL 18 可写主节点、目标允许连接且非模板、无自定义 ACL 或库级设置、UTF8 编码、维护角色为 superuser 或“目标 owner + CREATEDB”、零活动会话、无预备事务、无活动逻辑复制槽与订阅。通过后执行：

```bash
auction-cli --config-dir /etc/auction db rebuild --database auction \
  --maintenance-secrets /etc/auction/maintenance/postgres.toml \
  --mode empty-for-restore \
  --backup-manifest /var/backups/auction/auction-<时间>.dump.manifest \
  --execute --confirm 'REBUILD auction' \
  --receipt /var/backups/auction/rebuild-<时间>.json
```

命令以原 owner、编码、locale provider、tablespace 与连接上限重建空库（`DROP DATABASE … WITH (FORCE)` + `CREATE DATABASE … TEMPLATE template0`），并写入回执（`auction-postgres-rebuild.v1`）。

目标库已不存在（例如新集群）时，改用 `auction-cli db create --database auction --maintenance-secrets …` 建立空库。**不要**在恢复前执行 `db migrate`——恢复目标必须没有任何 schema。

### 3. 预检与演练

```bash
auction-cli --config-dir /etc/auction db preflight --database auction \
  --output /var/backups/auction/preflight-<时间>.json
auction-cli --config-dir /etc/auction backup restore --database auction \
  --manifest /var/backups/auction/auction-<时间>.dump.manifest --dry-run
```

`restore_preflight_passed = true` 只表示目标是“空库、无其他连接、可写主节点”，不代表备份可用；dry-run 同时校验 archive。两者都不能阻止外部客户端在检查后重新连接。

### 4. 恢复

```bash
auction-cli --config-dir /etc/auction backup restore --database auction \
  --manifest /var/backups/auction/auction-<时间>.dump.manifest \
  --receipt /var/backups/auction/restore-<时间>.json
```

- 恢复前再次校验 archive，以 `pg_restore --single-transaction --exit-on-error --no-owner --no-privileges` 执行，完成后核对 schema 身份与迁移版本等于备份版本，最后写入回执（`auction-postgres-restore.v1`）。
- 恢复使用单事务；明确在提交前失败时应整体回滚，但客户端报错、连接中断或回执缺失都不能单独证明目标仍为空。保持停机，用 `db preflight` 核对；若恢复已提交但后续核对或回执写入失败，库已非空，不能直接重跑恢复，须先确定实际结果。
- 备份的 schema 版本不得高于当前 `auction-cli`；恢复不保留对象 owner 与权限（空库归应用角色所有，恢复后的对象也归它所有）。

### 5. 迁移

```bash
auction-cli --config-dir /etc/auction db status        # 备份版本较旧时显示未到当前版本
auction-cli --config-dir /etc/auction db migrate       # 仍保持三角色停止
```

合并部署：以服务账号执行 `auction-cli --config-dir /var/lib/auction/config db migrate`；完成下述外部历史核对后再启动。

### 6. 外部历史核对与启动

数据库回到了备份时刻，但外部世界没有：备份之后 Bot 可能已经寄出过邮件、收取过附件或发过帖子。恢复后的数据库会认为这些动作尚未发生或仍在进行中。再次启动 Worker 前，应先结合运维记录、数据库只读查询与远端已有证据核对备份后的外部动作，确定可以恢复自动处理；需要用 Admin 页面进一步检查时，应先在部署配置中停用相关外部功能并核对 runtime 接管值。

- 检查进行中与结果未知的投递/履约：过期 `sending` 先隔离为 `manual_reconcile`，邮件维护只尝试精确 Sent 证明恢复，不自动重发未知 Send；但在备份时刻尚未开始的投递会被正常发送，仍须对照备份后的 Bot Sent 记录排除重复寄出。
- 检查邮件轮询游标与待读项：详情读取会把邮件移出未读 Inbox，备份之后已处理的邮件未必会自动被重新发现。去重依赖恢复库中已有的“邮箱 + 远端邮件 ID”记录；须人工核对备份之后接受的投稿、已收取附件及处理记录，不能假定恢复旧游标就会安全重放全部入站邮件。
- 检查论坛主题（`auction-cli forum recovery`）与论坛私信（`auction-cli pm list`）。
- 确定可以恢复后，分体部署执行 `systemctl start auction-stack.service`，合并部署执行 `systemctl start auction-server.service`；通过 Admin “投递队列”“结算报表”复核，并只在核对完成后恢复之前停用的外部功能。

## 演练

定期在**隔离的 PostgreSQL 实例**上演练完整恢复。使用同名数据库（manifest 绑定库名摘要，不能恢复到其他库名）。一次性演练库可用 `db rebuild --ephemeral` 替代备份前提做 `fresh-schema` 重建，实际执行仍须提供 `--execute --confirm 'EPHEMERAL <库名>' --receipt <新回执>`；它不会生成恢复所需的空库，`empty-for-restore` 模式必须提供 `--backup-manifest` 且不接受 `--ephemeral`。

含迁移版本部署前，还须按[迁移演练](MIGRATION-DRILL.md)用最新备份和候选程序完成升级、readiness 并留存[回执](migration-drill-receipt.template.json)。恢复演练与新空库安装不能替代旧备份到候选 schema 的升级演练；该回执中的完整停机耗时用于设置 CLI 时段保护窗口。

## 全新空库

需要一个带当前 schema 的全新库（例如测试环境）时：

```bash
auction-cli db rebuild --database auction --maintenance-secrets … \
  --mode fresh-schema --backup-manifest <现有库的备份 manifest> \
  --execute --confirm 'REBUILD auction' --receipt <回执>
```

或对新建的空库直接执行 `auction-cli db migrate`。

## 回执状态判读

| 字段 | 含义 |
| --- | --- |
| `outcome` | 重建回执的整体结果：`completed` 或 `failed` |
| `rebuild` | DROP/CREATE 已成功时的原/新数据库 OID、owner 与建库参数；后续迁移仍可能失败 |
| `failure.stage` | 失败阶段，如 `drop_database`、`create_database`、`verify_recreated_database` 或 `migrate` |
| `failure.database_dropped` / `failure.database_recreated` | 失败时记录的已知破坏性状态；`true`/`false` 为确定结果，`null` 为无法确认；部分预检失败只记录 `database_dropped = false` |

出现失败或未知时：保持全部角色停止，用 `db preflight` 与 `psql` 核对目标库是否存在、是否为空、OID 是否变化，再决定是重新准备空库还是直接恢复。命令进程若在 DROP 之后被强制终止，可能来不及写回执，此时以数据库 catalog 与备份为最终依据。

## 数据保留

`auction-cli db inventory` 按[保留政策](retention-policy.v1.json)统计各数据族的规模，只读、不授权任何归档或删除。当前政策对业务数据全部为永久保留；任何清理都需要先由业务负责人确定保留期限、法律保留与归档方案，再单独实现与评审。
