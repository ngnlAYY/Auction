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

发布包的分体与合并形态均包含 `systemd/auction-backup.service.example` 与 `systemd/auction-backup.timer.example`（仓库源为 `ops/deploy/systemd/` 下同名文件），以及 WAL 上传器的 `systemd/auction-wal-upload.service.example`、`systemd/auction-wal-upload.timer.example` 与每周基础备份的 `systemd/auction-base-backup.service.example`、`systemd/auction-base-backup.timer.example`。启用由用户执行；启用 timer 同时授权按下述规则清理这个专用目录内的旧定时备份。备份范围仍只有数据库；可选异机副本将 archive 与 manifest 分别加密上传 R2。runtime 配置与诊断证据由后续工作项交付；WAL 归档、每周基础备份与上传见[WAL 归档与时间点恢复](#wal-归档与时间点恢复)。

timer 的 `ExecStart` 调用 `auction-cli backup scheduled`；线上旧 CLI `f99a9d4e` 早于该子命令，**必须先安装包含该命令的候选 CLI，再启用 timer**。R2、`age` 与监控服务可以在部署前准备，但旧程序上的 timer/出口验收不成立。完整顺序见[ops/README.md#m1-候选上线顺序备份与指标前置](../README.md#m1-候选上线顺序备份与指标前置)。

先确认发布包中的 `auction-cli` 带 postgres feature，运行账号能读取配置，PATH 中的 `pg_dump/pg_restore` 为 PG18 客户端；启用异机副本还须按下节安装主机 `age` 并完成离线托管及 bucket lock 设置。根据实际库名和目录编辑 service。以下安装命令在解压后的发布包根目录以 root 执行：

```bash
install -d -o auction -g auction -m 0700 /var/backups/auction/scheduled /var/lib/auction/backup
install -m 0644 systemd/auction-backup.service.example /etc/systemd/system/auction-backup.service
install -m 0644 systemd/auction-backup.timer.example /etc/systemd/system/auction-backup.timer
sudo -u auction /opt/auction/bin/auction-cli --config-dir /etc/auction config check --strict
systemctl daemon-reload
systemctl enable --now auction-backup.timer
```

合并部署将 service 的 `--config-dir` 改为 `/var/lib/auction/config`。两个目录必须是不重叠的私有真实目录，运行账号固定；状态目录保存历史回执与运行记录，不要因更新程序而删除。普通参数在 `common.toml [backup]`，见[ops/config/README.md#定时备份策略](../config/README.md#定时备份策略)，修改后下一次 CLI 调用即生效，无需重启角色。停用回退为 `systemctl disable --now auction-backup.timer`；已经在执行的 oneshot 不因停用 timer 自动停止，若需中止由用户另行停止 service。停用不影响 Public/Admin/Worker，手动 create/verify/restore 继续可用。

默认排期由 `crates/auction-config/src/config/settings/auction/policy.rs` 的 `AuctionScheduleConfig::default` 与 `AuctionEconomicsConfig::default` 定义：UTC 周五 00:00 开拍，周六 12:00 名义结束，Final 提前 900 秒；`AuctionEarlyBirdConfig::default` 的早鸟及赛季前场次均在 18:00 名义结束。结算只在 Final 中所有 lot 的有效截止过去后启动（见[docs/guide/settlement.md#1-结算计算](../../docs/guide/settlement.md#1-结算计算)）。timer 选每日 UTC 03:15，随机延迟最多 15 分钟，CLI 最多两小时，避开这两组默认 Final/结算高峰。软截止延长、自定义排期和 Persistent 停机补跑可能改变实际重叠情况，部署者需按实际时段调整 timer。`Persistent=true` 只补一次错过的唤醒，不补造停机期间每天的备份。

任务先取得状态目录独占锁，再收口上次运行；取锁失败只写 stderr 并退出 3，不写回执。持锁后建立本次运行记录，只加载 Worker 及其引用文件并校验备份策略，Public/Admin 的无关配置不会阻断备份；联合 `config check --strict` 作为运维预检保留。总预算从命令启动计算（默认 7200 秒），创建、加密、旧项重试和日/月上传共用此期限，到期写 failed 并退出 3；活动项目记 timeout，顶层 failure 保留首个失败（没有更早错误时为 timeout）；service 的 130 分钟预算另给终态同步和打卡余量。磁盘在 create 前后检查，异机启用时还预留待写密文空间；暂存目录必须与输出同文件系统，已有暂存自然计入占用。按非特权可用空间判断，扣除预留后字节和比例都必须达标；不足记 `disk_low`，不通过删除仍应保留的恢复点腾空间。create、verify 或已启用的异机阶段失败都不进入本次本地清理。失败以退出码 3、终态回执和 journal 反映。

本地保留默认最近 7 份：保留本次已校验的新备份及最近 6 份合格旧备份。清理只认同一输出目录、库名摘要、scheduled 命名和 completed 回执，检查已校验标志、设备号/inode/大小及 owner-only 单链接普通文件；该次启用异机时还要求已校验的 daily 副本。旧 failed/interrupted 上传项重试成功后追加绑定原 manifest 的 `offsite/completed/<run_id>.json` 完成事实，供清理使用，不改写原失败/中断回执。旧 v1 未启用异机的 completed 回执保持原资格。删除前不重新 verify 或重读 archive；本地校验前失败的运行回执静默忽略，不反复记副本告警；已经本地校验但 daily 资格不足才记 replica_not_verified。实际异常、手动、不完整或缺少完成证据的目录项仍保留并按清理规则跳过。`offsite-staging/` 不进入保留枚举或跳过噪音，历史回执不清理。异机长期失败使本地备份和密文累积，最终触发 `disk_low`，属于失败关闭；用户应修复上传或增加容量，不自动扩大删除范围。

查看 timer、service 与日志：

```bash
systemctl list-timers auction-backup.timer
systemctl status auction-backup.service
journalctl -u auction-backup.service
```

每次运行的回执为 `/var/lib/auction/backup/receipts/<run_id>.json`，格式 `auction-backup-run.v1`，字段及错误码见[apps/auction-cli/README.md#定时备份](../../apps/auction-cli/README.md#定时备份)。回执为 0600、临时文件同步后无覆盖原子改名发布，`completed/failed/interrupted` 都是不可变终态；运行中状态为 `<state-dir>/running.json`，阶段更新使用原子替换。状态目录锁排除并发进程，无记录租约。强杀或断电当次可能来不及写终态，当次只依赖 systemd 失败状态和 journal；下一次 scheduled 取得同一目录锁后，对有效残留记录追加 interrupted。终态已发布且身份、已知 archive 证据一致时沿用原终态，不改成中断。无效记录或与同名终态冲突的记录，以 0600、`RENAME_NOREPLACE` 原样改名保留为 `<state-dir>/running.invalid-<UTC时间戳>-<本次run_id>.json`；本次回执增加 `recovery_warnings`，journal 告警只显示原因码及保留文件名，随后照常执行备份。保留文件需人工查看，历史终态不修改。只有存储读写或保留改名失败才报告 `recovery_failed` 并停止。不要直接把残留 archive 当作已校验备份。

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

仍未落地：定期把范围材料纳入恢复演练（W10.3）、恢复后外部核对工具（W10.9）与自动引用回执的发布门禁（W7.2）。当前只有手工 `scope-extract` 与 `backup verify` 的摘要校验，不能据此声明端到端演练完成。

## WAL 归档与时间点恢复

目标（决策 18、31、41、49）：RPO ≤ 5 分钟（`archive_timeout = 300`）、RTO ≤ 2 小时，可恢复到过去 **14 天**内任一时刻；WAL 与基础备份都保留 **21 天**，同时保有 3 份基础备份。两者使用 R2 的独立前缀 `<prefix>/wal/` 与 `<prefix>/base/`，生命周期与 bucket lock 均为 21 天，与日/月备份前缀互不影响。

本仓库当前交付的切片：`archive_mode`/`archive_timeout` 模板、`archive_command` 调用的 `auction-cli wal archive`（zstd 压缩 + age 公钥加密 + fsync 暂存）、每周 `auction-cli wal base-backup`（`pg_basebackup` 物理基础备份，同样 zstd + age + fsync 暂存）以及独立上传器 `auction-cli wal upload`（R2 `wal/` 与 `base/` 前缀、长度与完整 SHA-256 校验）。WAL 校验通过后删除本地副本；基础备份在本地保留最近 **3** 份、更旧的完整暂存项在校验上传后删除，远端份数由 bucket 生命周期（21 天）淘汰。归档、上传与暂存目录大小的指标和外部打卡尚未实现，现阶段靠 `pg_stat_archiver`、`systemctl` 状态与磁盘占用观察。**本仓库尚未在隔离实例上完成一次真实的时间点恢复演练**，时间点恢复步骤见下文，交付状态不能当作演练结论。

### 组成与数据流

1. PostgreSQL archiver 在段写满或 `archive_timeout` 到期后调用：

   ```conf
   archive_command = '/opt/auction/bin/auction-cli wal archive --staging-dir /var/lib/auction/wal-staging --recipient age1<收件人公钥> %p %f'
   ```

   命令以 **postgres 账号**执行，不读敏感 TOML；收件人公钥写在参数里（公钥不是秘密，私钥仍按[离线生成与托管密钥](#1-离线生成与托管密钥)保存）。它把 `zstd` 压缩流经 `age --encrypt --recipient` 写入 `<段名>.zst.age`，`fsync` 文件与目录后发布 `<段名>.zst.age.manifest.json`（`auction-wal-archive.v1`：段名、源字节数与 mtime、密文长度、归档时刻）。任一步失败返回非零，由 PostgreSQL 重试同一段；同一段已有 manifest 且源文件字节数与 mtime 一致时直接成功退出，不重复加密。源内容不同或密文长度与 manifest 不符时失败关闭，不覆盖既有证据。
2. **每周基础备份**由 `auction-base-backup.timer` 以 **postgres 账号**执行 `auction-cli wal base-backup --staging-dir /var/lib/auction/wal-staging --recipient age1<收件人公钥>`。它运行 `pg_basebackup --pgdata=- --format=tar --wal-method=fetch --checkpoint=fast --label base-<UTC时刻> --no-password`，连接的数据库与角色来自 libpq 默认值（本机 socket、peer 认证），因此执行账号必须有 `REPLICATION` 权限或为超级用户；`-D -` 要求集群**没有额外表空间**，`-X fetch` 把所需 WAL 并入同一个 `base.tar` 流，使产物是可直接用于时间点恢复的独立基础备份。tar 流经 `zstd` 与 `age --encrypt --recipient` 写入 `base-<UTC时刻>.tar.zst.age`，`fsync` 文件与目录后发布 `base-<UTC时刻>.tar.zst.age.manifest.json`（`auction-base-backup.v1`：备份 ID、开始/结束时刻、压缩前与密文字节数、密文 SHA-256）。任一外部程序非零退出都失败关闭，不发布任何文件；同一秒内重跑沿用既有身份，只有密文而没有 manifest 视为上次中断并重新归档。
3. `auction-wal-upload.timer` 每 5 分钟以 **auction 账号**运行 `auction-cli wal upload --staging-dir /var/lib/auction/wal-staging --budget-seconds 300`。WAL 按段名顺序上传到 `<prefix>/wal/<段名>.zst.age`，校验通过后删除本地密文与 manifest。基础备份上传到 `<prefix>/base/<备份 ID>/`：先密文、再 manifest，最后发布 `completed.json`（`auction-base-upload.v1`：备份 ID、备份完成时刻、两份对象的长度与 SHA-256）；完成记录取备份完成时刻而非上传时刻，响应丢失后重试生成完全相同的字节。远端已有相同长度与完整 SHA-256 的对象视为成功（bucket lock 下重传会返回 409）。基础备份/WAL 共用一次运行的总预算；单份密文无法在 300 秒内上传完时，同时调大 `--budget-seconds` 与 service 的 `TimeoutStartSec`，否则每次都会超时并保留本地文件重试。
4. 基础备份每次成功上传后，上传器在暂存目录写入本机证据 `base-<备份 ID>.upload.json`（`auction-base-receipt.v1`：备份 ID、上传时刻、`completed.json` 的 SHA-256）。之后每 5 分钟的运行据此跳过该组，不再重算本地 SHA-256 或完整回读远端；证据缺失或无效时按未上传处理，宁可多一次完整校验。删除该文件就会让下一次运行重新核对远端对象（幂等，不会产生第二份）。
5. 基础备份的本地保留在每次上传运行末尾执行：按备份 ID 倒序保留最新 **3** 组完整暂存项（密文与 manifest 长度一致），更旧的完整项依次删除本机证据、manifest 与密文，并 `fsync` 目录；本次上传失败的旧备份不会被删除，未形成完整一对的暂存项只报告 `incomplete`、从不主动删除。异机副本关闭（`[backup.offsite] enabled = false`）时保留策略照常执行，此时本地就是唯一副本，报告 `outcome = skipped`。
6. 上传失败、冲突或预算耗尽都保留本地文件并以非零退出；stdout 的 JSON 报告逐项给出 WAL 的 `uploaded`、`already_present`、`failed`、`invalid`、`incomplete` 与 `remaining`，以及基础备份的 `base.uploaded`、`base.already_present`、`base.already_verified`、`base.failed`、`base.retained`、`base.removed`，systemd 记录失败状态。`incomplete` 是创建进程尚未发布完成的项（WAL 段名或基础备份 ID），不是上传失败。

### 启用顺序

1. 安装包含 `wal archive`/`wal base-backup`/`wal upload` 的 `auction-cli`（旧 CLI 没有这些子命令，先装候选再改 PostgreSQL）。
2. 在归档账号（postgres）的 PATH 中安装 `zstd`、`age` 与 `pg_basebackup`（PG18 客户端），确认 `zstd --version`、`age --version`、`pg_basebackup --version` 可执行；服务器只放公钥。同时确认 postgres 账号可执行 `/opt/auction/bin/auction-cli`（目录可遍历、二进制可执行），否则 `archive_command` 直接返回 127。
3. 建立暂存目录并确认权限：`postgres:auction`、`2770`（setgid，使密文继承 `auction` 组）。可用 `ops/deploy/tmpfiles/hv-auction.conf.example` 中对应的 `d /var/lib/auction/wal-staging 2770 postgres auction - -` 行，或手工 `install -d -o postgres -g auction -m 2770 /var/lib/auction/wal-staging`。WAL 段与基础备份共用该目录；基础备份会占用本机数份容量。
4. 以 auction 账号执行 `auction-cli --config-dir /etc/auction config check --strict`，确认 `[backup.offsite]` 已启用且 R2 令牌可写 `<prefix>/wal/` 与 `<prefix>/base/`。
5. 手工验证一次归档与上传（不改 `archive_mode`）：

   ```bash
   sudo -u postgres /opt/auction/bin/auction-cli wal archive \
     --staging-dir /var/lib/auction/wal-staging --recipient age1<收件人公钥> \
     /path/to/pg_wal/000000010000000000000001 000000010000000000000001
   sudo -u auction /opt/auction/bin/auction-cli --config-dir /etc/auction wal upload \
     --staging-dir /var/lib/auction/wal-staging --budget-seconds 300
   ```

   重跑同一 `wal archive` 必须输出 `already_archived` 并退出 0；重跑上传必须报 `already_present` 或空集合。
6. 手工验证一次基础备份与上传：

   ```bash
   sudo -u postgres /opt/auction/bin/auction-cli wal base-backup \
     --staging-dir /var/lib/auction/wal-staging --recipient age1<收件人公钥>
   sudo -u auction /opt/auction/bin/auction-cli --config-dir /etc/auction wal upload \
     --staging-dir /var/lib/auction/wal-staging --budget-seconds 300
   ```

   第一条输出 `status = created`，暂存目录出现同名 `.tar.zst.age` 与 `.manifest.json`；第二条报告 `base.uploaded`（或 `base.already_present`）且 R2 `<prefix>/base/<备份 ID>/` 下三份对象齐全。同一天再次运行 `base-backup` 会创建新的备份 ID，这是预期行为；保留策略只在超过 3 份时删除最旧的完整副本。执行账号必须有 `REPLICATION` 权限或为超级用户，集群不能有额外表空间。
7. 安装并启用 `auction-wal-upload.service/timer` 与 `auction-base-backup.service/timer`（发布包 `systemd/` 目录），确认各一次成功运行。上传器同时负责 WAL 与基础备份；只启用基础备份 timer 而不启用上传 timer 时，基础备份只在本地暂存且不会按远端校验后清理。
8. **只有以上步骤都通过后**，才在 `postgresql.conf` 打开归档并在 W7.5 允许的窗口重启：

   ```conf
   archive_mode = on
   archive_timeout = 300
   ```

   开启 `archive_mode` 需要重启 PostgreSQL，受[apps/auction-cli/README.md#停机时段门禁](../../apps/auction-cli/README.md#停机时段门禁)约束；不能用 `pg_ctl reload` 代替，也不要在 Final 或临近截止的窗口执行。

### 验证

```bash
psql -d auction -c 'SELECT archived_count, failed_count, last_archived_wal, last_failed_wal FROM pg_stat_archiver'
systemctl list-timers auction-wal-upload.timer auction-base-backup.timer
systemctl status auction-wal-upload.service auction-base-backup.service
journalctl -u auction-wal-upload.service -u auction-base-backup.service
df -h /var/lib/auction/wal-staging
ls -l /var/lib/auction/wal-staging | grep '^.*base-'
```

`failed_count` 持续增长表示 `archive_command` 失败（zstd/age 缺失、权限、暂存目录不可写），此时 `pg_wal` 会增长；暂存目录长期非空表示上传器失败或未启用 timer。R2 侧核对 `<prefix>/wal/` 下对象数量与最新段名，以及 `<prefix>/base/` 下每个备份目录是否都有 `completed.json`；不与日/月备份混用前缀。本机基础备份数量应稳定在 3 组，每组三份文件（密文、manifest 与该备份上传成功后出现的 `.upload.json` 本机证据）；只有密文与 manifest 而没有证据表示还没成功上传，只有证据与 manifest 没有密文说明该组已被清理或损坏。

### 失败处置

| 现象 | 处置 |
| --- | --- |
| `pg_stat_archiver.failed_count` 增长，journal 出现 `zstd 压缩失败` / `age 加密失败` | 以 postgres 账号手工执行一次 `wal archive`，核对 PATH、公钥与暂存目录权限；修复后 PostgreSQL 自动重试同一段，不需要手工删 `pg_wal` |
| `WAL manifest 存在但密文缺失` | 保留 manifest 与目录，人工核对是否有人删除了密文；确认密文确实不存在且该段尚未被确认时，删除残留 manifest 让下一次重试重新归档 |
| `同名 WAL 段已有不同源内容的归档` | 失败关闭，不覆盖。核对段名是否被另一实例复用、暂存目录是否被复制；处理前先保留全部证据 |
| 上传器 `offsite_object_conflict` | 远端同名对象长度或摘要不同：保留本地密文，按[异机异常处置](#异机异常处置)核对 bucket、prefix 与令牌，不覆盖、不改名 |
| `wal base-backup` 报 `pg_basebackup 失败` | 读取 journal 中截断的 stderr：连接失败、缺少 `REPLICATION` 权限、额外表空间或 WAL 已被回收都会在这里失败。修复后用 postgres 账号手工重跑；命令不发布任何半成品 |
| `wal base-backup` 报 `基础备份 manifest 存在但密文缺失` | 失败关闭：保留 manifest 与目录，人工核对是否有人删除了密文；确认密文确实不存在时，删除残留 manifest（或整组暂存）后重新生成基础备份。不能手工补写 manifest |
| `基础备份密文 SHA-256 与 manifest 不一致` | 上传器在上传前失败关闭。该暂存项已损坏，不能上传或改名规避；保留证据并由用户决定删除该组后重新生成，不能在保留份数外长期占用暂存目录 |
| 上传器 `base.failed` 长期非空 | 按报告中的备份 ID 核对 R2 前缀、bucket lock 与令牌；本地副本会保留（本次上传失败的项不参与保留删除），修复后下一次上传按对象摘要对账并输出 `base.already_present` |
| 怀疑远端基础备份被删除或被改动 | 本机 `.upload.json` 只是“曾经校验通过”的证据，不会每次运行交叉重核远端。删除对应证据后下一次上传会重新 HEAD + 完整回读并输出 `base.already_present`（不再产生第二份）；对象缺失时重新上传。远端对象的淘汰只由 bucket 生命周期执行，CLI 从不删除远端对象 |
| 暂存目录占满磁盘 | 先确认上传器与 R2 可用并恢复上传，不要直接删除未上传密文；确需腾空间时按授权范围处理已上传且校验通过的对象 |
| 需要停止归档 | 先 `systemctl disable --now auction-wal-upload.timer`，再在 W7.5 窗口内把 `archive_mode` 改回 `off` 并重启；停机期间未归档的段留在 `pg_wal`，重启后 PostgreSQL 会继续尝试归档 |

### 时间点恢复

完整流程需要 R2 中的一份基础备份与截至目标时刻的全部 WAL。基础备份与归档命令已交付，但**本仓库尚未在隔离 PG18 实例上完整演练过该流程**；执行一次演练前，恢复点仍以[定时备份](#定时备份)的日/月逻辑备份为主，它只能恢复到备份时刻。

1. 在 R2 的 `<prefix>/base/<备份 ID>/` 中选择备份，下载 `completed.json` 检查 `format = auction-base-upload.v1`、备份 ID、`created_unix_seconds`、两份对象名/长度/SHA-256，并核对其中的 manifest 与暂存时 manifest 一致；只有密文而没有完成记录时不能当作恢复点。随后下载该备份的 `base-<ID>.tar.zst.age` 与 manifest，再下载 `<prefix>/wal/` 中目标时刻之前、且晚于该基础备份起点的全部段。
2. 在离线工作站用 age 私钥把基础备份与 WAL 段解密到 0700 目录，逐个用 `wc -c` 与 `sha256sum` 核对完成的记录与 `auction-base-backup.v1` 的 `cipher_bytes`/`cipher_sha256`；再用 `zstd -d` 还原 `base.tar` 与 `<段名>` 文件名。不得在恢复机留下私钥或明文 WAL。
3. 把 `base.tar` 展开为隔离实例的数据目录（tar 内已包含备份期间的 WAL，可直接启动），写入指向解密后 WAL 目录的 `restore_command`，创建 `recovery.signal`，在 `recovery_target_time` 指定目标时刻，启动实例并核对停止点日志。`-X fetch` 的基础备份只包含备份期间的 WAL，之后的时间点仍依赖 `wal/` 中已归档的段。
4. 核对 schema/迁移版本、关键表与只读业务事实，然后按[外部历史核对](#6-外部历史核对与启动)排除备份之后已经发生的邮件、附件与论坛写入；恢复库不能直接接生产外部功能。
5. 记录使用的备份 ID 与段范围、目标时刻、解密与还原结果、恢复耗时与外部核对结论。一次演练习得后才能把时间点恢复计入可恢复性证据。


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
2. 按下表为对象键前缀配置按对象年龄淘汰的生命周期和 bucket lock；启用备份之前核验规则已生效。`<prefix>` 为空时键直接为 `daily/`、`monthly/` 等，非空时为 `<prefix>/daily/`。锁定期不长于淘汰期，按决策使用相同天数。`<prefix>/wal/` 与 `<prefix>/base/` 由 W10.7 上传器产生（见[WAL 归档与时间点恢复](#wal-归档与时间点恢复)），`base/` 下每个备份一个目录（`<prefix>/base/<备份 ID>/`）；默认 `archive_mode = off`，启用须按该节顺序执行。

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
2. 范围证据（启用时）：完成记录的 `scope` 字段给出 `<archive>.scope.bundle.age` 与 `<archive>.scope.json` 两个对象，同样核对长度与 SHA-256 并保持原文件名。范围材料留在离线工作站，等第 4 步解密 manifest 后再执行

   ```bash
   auction-cli backup scope-extract --manifest <解密后的manifest> --database <原库名> \
     --identity <age 私钥> --output-dir <全新私有目录>
   ```

   （需要 `PATH` 中有 `pg_restore`，与 `backup verify` 相同）。解出的运行配置、邮件诊断证据与部署回执用于重建主机；不要在生产备份服务器上保留 identity 或解出目录。

3. 在受信任工作站通过控制台或已授权的 S3 客户端下载**记录指定的两个对象**，保持 `<archive>.age` 与 `<archive>.manifest.age` 文件名。下载目录为 0700，文件为 0600。逐一用 `wc -c < 文件` 和 `sha256sum 文件` 核对记录的 bytes 与 sha256；不能使用 multipart ETag 代替整文件摘要。任一不符则保留证据并停止该恢复点。
4. 按上节从密码管理器或离线副本取回对应 age 私钥，在离线工作站解密到新文件，禁止覆盖已有恢复证据：

   ```bash
   umask 077
   age --decrypt --identity backup-identity.txt --output scheduled-<时间>-<run_id>.dump \
     scheduled-<时间>-<run_id>.dump.age
   age --decrypt --identity backup-identity.txt --output scheduled-<时间>-<run_id>.dump.manifest \
     scheduled-<时间>-<run_id>.dump.manifest.age
   sha256sum scheduled-<时间>-<run_id>.dump.manifest
   ```

   manifest 明文摘要必须与完成记录的 `plaintext_manifest_sha256` 一致；两次解密认证均成功后，再安全传递 archive/manifest。不要把 identity 文件放到生产备份服务器。
5. 使用与 manifest 绑定的**原库名**执行离线校验：`auction-cli backup verify --database <原库名> --manifest <解密后的manifest>`。隔离配置指向 PG18 独立端口/实例，三角色保持停止；准备空库，执行 `db preflight`、`backup restore --dry-run`，再执行 `backup restore --receipt <新的0600回执>`，具体门禁和参数见[恢复流程](#恢复流程)。不在恢复前 migrate，不用 fresh-schema 代替空库。
6. 从托管清单取回对应历史版本的三组数据密钥，在隔离环境只读读取恢复库中的样本信封，使用现有 owner 的解密入口核验：装备 `EquipmentKeyOpener::open`，用户登录 key `MoogleMailLoginKeyReader` 的 `AdminLoginKeyOpener::open`，外部 Cookie `CredentialSealer` 的 `AdminCredentialOpener::open`。保留原装备/用户/凭据 ID、kind、scope、key_id 和 AAD 绑定，验证认证标签与指纹；不能通过改信封、重新封装或重新注册来证明旧材料可恢复。由监督者授权的隔离核验程序调用这些入口，只输出数据族、样本 ID、key_id、成功条数和失败分类，不输出密钥、Cookie、登录 key 或装备明文；当前 CLI 没有通用“导出解密列”命令。
7. 核对恢复 schema/迁移版本、关键表和只读业务事实，记录完成记录自身摘要、R2 对象名、下载校验结果、restore 回执、三组材料的解密条数和演练耗时。恢复到生产前仍须执行[外部历史核对](#6-外部历史核对与启动)，不因一次隔离演练而自动放行外部重发。清理恢复工作副本按托管流程执行，长期保留非秘密演练回执。

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

### 用 auction-cli rollback 换回旧程序

W7.2 的 `deploy` 在安装前把当前四个程序保留到 `<state-dir>/rollback/<deploy_receipt_id>/`（`record.json` 记录安装目录、`previous_version`、`deployed_version` 与每个程序的 SHA-256）。`auction-cli rollback --receipt-id <deploy_receipt_id>`（省略则取最新记录）只换回该记录中的程序，并且：

- 要求安装目录中的程序仍是该记录对应部署回执里的版本；安装目录与记录不一致时拒绝，先核对 unit `ExecStart`。
- 读取数据库版本，要求 `previous_version → 当前版本` 的每个迁移连续、成功且登记为 `compatible`；任一 `breaking` 或缺少登记即拒绝——此时只能按本文件恢复升级前备份并做外部历史核对。
- 与 `deploy` 相同，先过 W7.5 时段门禁与在途发送门禁，再按 Public → Admin → Worker 停止；安装前把当前程序再次保留为新的回滚记录，因此回退可以重新前进。
- 启动（不执行迁移）后核对三个运行程序的摘要，成功追加 `action=rollback` 回执。

`--dry-run` 只核对记录、安装目录、兼容区间与门禁，不停止服务。手工换回旧程序仍须显式设置 `[readiness] migration_compatibility_mode = "compatible"`，并保留能识别当前数据库的新版 CLI；默认 shadow 不放宽。


迁移只向前，不执行 down、不删除 `_sqlx_migrations` 或兼容登记。令 `E` 为目标旧程序内嵌的最高迁移版本，`D` 为当前数据库最高版本，只核对本次回退跨越的 `(E,D]` 区间：

| 区间事实 | 可用路径 |
| --- | --- |
| `D = E`，完整历史严格匹配 | 按原流程换回同 schema 的旧程序 |
| `D > E`，已知前缀严格匹配，`E+1…D` 连续成功且逐版本登记 `compatible` | 目标程序已包含 W7.8a，并显式启用 compatible 后，可保留当前数据库换回程序 |
| 区间含 breaking、缺登记、登记未知、迁移失败或无法确认 | 拒绝直接换回；恢复升级前备份，并执行下述外部历史核对 |
| `D < E` | 目标程序要求尚未完成的迁移，不能通过兼容开关放行 |

001–074 不回填且缺行即 breaking，075 自身也登记 breaking。兼容能力须先随版本 A 部署，之后版本 B 才能换回 A；不能因此回到尚不含该逻辑的程序。登记表及精确判定由[docs/contracts/database.md#迁移兼容登记与就绪](../../docs/contracts/database.md#迁移兼容登记与就绪)定义。

保留数据库的回退步骤由运维在维护窗口受控执行：

1. 确认目标制品、其内嵌最高版本 E、配置兼容性与可用备份，按[apps/auction-cli/README.md#停机时段门禁](../../apps/auction-cli/README.md#停机时段门禁)停止全部角色；合并部署停止 supervisor 并等待子进程退出。
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
  --owner-secrets /etc/auction/owner/postgres.toml \
  --mode empty-for-restore \
  --backup-manifest /var/backups/auction/auction-<时间>.dump.manifest
```

演练先校验备份的 manifest、archive 摘要与目录，并要求备份 schema 版本不高于当前 `auction-cli` 支持的版本；此版本门禁在任何 DROP/CREATE 前执行，离线 `backup verify` 通过本身不代表当前工具支持恢复该版本。随后持锁检查：维护库为 `postgres`、PostgreSQL 18 可写主节点、目标允许连接且非模板、无自定义 ACL 或库级设置、UTF8 编码、维护角色为 superuser 或“目标 owner + CREATEDB”、零活动会话、无预备事务、无活动逻辑复制槽与订阅。通过后执行：

```bash
auction-cli --config-dir /etc/auction db rebuild --database auction \
  --maintenance-secrets /etc/auction/maintenance/postgres.toml \
  --owner-secrets /etc/auction/owner/postgres.toml \
  --mode empty-for-restore \
  --backup-manifest /var/backups/auction/auction-<时间>.dump.manifest \
  --execute --confirm 'REBUILD auction' \
  --receipt /var/backups/auction/rebuild-<时间>.json
```

命令以原 owner、编码、locale provider、tablespace 与连接上限重建空库（`DROP DATABASE … WITH (FORCE)` + `CREATE DATABASE … TEMPLATE template0`），并写入回执（`auction-postgres-rebuild.v1`）。

目标库已不存在（例如新集群）时，改用 `auction-cli db create --database auction --maintenance-secrets … --owner-secrets …` 建立空库；新库 owner 是 `--owner-secrets` 中的迁移 owner，服务账号（`auction_runtime`）不是库 owner。**不要**在恢复前执行 `db migrate`——恢复目标必须没有任何 schema。

### 3. 预检与演练

```bash
auction-cli --config-dir /etc/auction db preflight --database auction \
  --output /var/backups/auction/preflight-<时间>.json
auction-cli --config-dir /etc/auction backup restore --database auction \
  --owner-secrets /etc/auction/owner/postgres.toml \
  --manifest /var/backups/auction/auction-<时间>.dump.manifest --dry-run
```

`restore_preflight_passed = true` 只表示目标是“空库、无其他连接、可写主节点”，不代表备份可用；dry-run 同时校验 archive。两者都不能阻止外部客户端在检查后重新连接。

### 4. 恢复

```bash
auction-cli --config-dir /etc/auction backup restore --database auction \
  --owner-secrets /etc/auction/owner/postgres.toml \
  --manifest /var/backups/auction/auction-<时间>.dump.manifest \
  --receipt /var/backups/auction/restore-<时间>.json
```

- 恢复前再次校验 archive，以 `pg_restore --single-transaction --exit-on-error --no-owner --no-privileges` 执行，完成后核对 schema 身份与迁移版本等于备份版本，最后写入回执（`auction-postgres-restore.v1`）。
- 恢复使用单事务；明确在提交前失败时应整体回滚，但客户端报错、连接中断或回执缺失都不能单独证明目标仍为空。保持停机，用 `db preflight` 核对；若恢复已提交但后续核对或回执写入失败，库已非空，不能直接重跑恢复，须先确定实际结果。
- 备份的 schema 版本不得高于当前 `auction-cli`；恢复不保留对象 owner 与 ACL（`--no-owner --no-privileges`）。恢复后的对象归迁移 owner，回执的 `ownership_restored=false`。
- 备份版本已含迁移 `000081` 时，命令在核对 schema 后自动以同一 owner 连接重放 `public.apply_auction_runtime_privileges()`（回执 `runtime_privileges_applied=true`），否则服务账号没有对象权限。若该函数缺失或恢复的是更旧备份，命令保持停机并以非零退出，须先执行 `db migrate --owner-secrets` 再启动；手工换库或需要重新收口权限时执行 `auction-cli db grants --owner-secrets /etc/auction/owner/postgres.toml`。

### 5. 迁移

```bash
auction-cli --config-dir /etc/auction db status        # 备份版本较旧时显示未到当前版本
auction-cli --config-dir /etc/auction db grants --owner-secrets /etc/auction/owner/postgres.toml
auction-cli --config-dir /etc/auction db migrate --owner-secrets /etc/auction/owner/postgres.toml
```

复核权限重放（`granted` 信息见 stdout 的 `runtime_role`）：

```bash
sudo -u postgres psql -d auction -Atc "select has_schema_privilege('auction_runtime','public','CREATE'), has_table_privilege('auction_runtime','bids','UPDATE'), has_table_privilege('auction_runtime','bids','INSERT')"
# 期望：f|f|t
```

合并部署：以服务账号执行 `auction-cli --config-dir /var/lib/auction/config db migrate --owner-secrets /var/lib/auction/owner/postgres.toml`；完成下述外部历史核对后再启动。

### 6. 外部历史核对与启动

数据库回到了备份时刻，但外部世界没有：备份之后 Bot 可能已经寄出过邮件、收取过附件或发过帖子。恢复后的数据库会认为这些动作尚未发生或仍在进行中。再次启动 Worker 前，必须先暂停全部外部访问，再用 `auction-cli recovery external-reconcile` 做一次有界只读核对；工具只负责找出证据，不自动重发、领取或判断业务完成。

1. 发布 W1.9 暂停（带到期时间，到期自动恢复并留审计）：

   ```bash
   auction-cli --config-dir /etc/auction switch set external_access_paused true --until <RFC3339>
   ```

   Worker 处于暂停时 `recovery external-reconcile` 才是唯一允许的远端读取；未暂停时 Worker control 返回 `409 not_paused`，请求不发往远端。

2. 先只读演练并保存回执（`auction-external-recovery.v1`）：

   ```bash
   auction-cli --config-dir /etc/auction recovery external-reconcile \
     --cutoff <恢复点 manifest 的创建时间> --receipt /secure/recovery-<时间>.json
   ```

   工具经 Worker control 读取 Sent 与 Read Mail 摘要，只为列表投影匹配的候选 MID 读取详情，并读取最多 20 个已发布论坛主题的新楼层。回执的 `sent_complete`/`read_mail_complete`/`errors` 表示扫描是否覆盖到恢复点；`status=partial` 或截断时不能把“未观察到”当作“未发送”。

3. 核对回执中的 `deliveries.planned`、`read_mail.missing_locally` 与 `forum.missing_posts`。确认无误后执行隔离：

   ```bash
   auction-cli --config-dir /etc/auction recovery external-reconcile \
     --cutoff <同一恢复点> --receipt /secure/recovery-<时间>-applied.json \
     --apply --confirm 'APPLY EXTERNAL RECONCILIATION' --actor ops --scan-id <第一次回执的 scan_id>
   ```

   `--apply` 只把恢复库中仍为 `pending`/`claimed`、fence 未变且与 Sent 详情精确匹配的投递改为 `manual_reconcile`，并在同事务追加 `delivery_external_quarantines` 与审计；不会重发、领取、改写资金/结算，也不代表收件人已领取。重复扫描或同 `scan_id` 重放幂等。Admin 仍用既有 `manual_reconcile` 动作人工处置。

4. 工具不能自动完成的部分仍需人工核对：

   - 备份后已收取的附件与 COD 收款：读取恢复库、原始邮件诊断证据与游戏内实际领取记录，逐笔确认；隔离只表示“备份后可能已寄出”，资金事实仍按真实领取证明收口。
   - 恢复库缺失的已读入站邮件：按 MID、发件人、标题与时间人工核对投稿/回执/换绑是否已在备份后处理；不能假定旧游标会安全重放。
   - 缺失的论坛新楼层：按 `thread_id`/`post_id` 与 `auction-cli forum recovery`、`auction-cli pm list` 核对是否产生过竞价、`notify` 或私信；缺失楼层可能包含未入库的玩家命令。
   - 任何未知 Send 仍保持 `manual_reconcile`；只有原 Sent 详情精确证明或 `settlement confirm-sent` 人工证明才能收口，绝不自动重发。

5. 确定可以恢复后，先解除暂停（或等待到期），再分体执行 `systemctl start auction-stack.service`、合并执行 `systemctl start auction-server.service`；通过 Admin “投递队列”“结算报表”复核，并只在核对完成后恢复之前停用的外部功能。

## 演练

定期在**隔离的 PostgreSQL 实例**上演练完整恢复。使用同名数据库（manifest 绑定库名摘要，不能恢复到其他库名）。一次性演练库可用 `db rebuild --ephemeral` 替代备份前提做 `fresh-schema` 重建，实际执行仍须提供 `--execute --confirm 'EPHEMERAL <库名>' --receipt <新回执>`；它不会生成恢复所需的空库，`empty-for-restore` 模式必须提供 `--backup-manifest` 且不接受 `--ephemeral`。

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

| 字段 | 含义 |
| --- | --- |
| `outcome` | 重建回执的整体结果：`completed` 或 `failed` |
| `rebuild` | DROP/CREATE 已成功时的原/新数据库 OID、owner 与建库参数；后续迁移仍可能失败 |
| `failure.stage` | 失败阶段，如 `drop_database`、`create_database`、`verify_recreated_database` 或 `migrate` |
| `failure.database_dropped` / `failure.database_recreated` | 失败时记录的已知破坏性状态；`true`/`false` 为确定结果，`null` 为无法确认；部分预检失败只记录 `database_dropped = false` |

出现失败或未知时：保持全部角色停止，用 `db preflight` 与 `psql` 核对目标库是否存在、是否为空、OID 是否变化，再决定是重新准备空库还是直接恢复。命令进程若在 DROP 之后被强制终止，可能来不及写回执，此时以数据库 catalog 与备份为最终依据。

## 数据保留

`auction-cli db inventory` 按[ops/deploy/retention-policy.v2.json](retention-policy.v2.json)统计各数据族的规模，只读、不授权任何归档或删除。`auction-cli db retention-dry-run` 按政策期限统计运行态族的候选行与加密诊断文件候选数，只执行 SELECT 并输出带 `run_id` 的私有回执；`mutation_executed`/`deletion_authorized` 恒为 false。审计、资金与业务事实永久保留。

实际清理只经 `auction-cli db retention-purge`：它绑定 15 分钟内、当前政策身份的演练回执与逐数据族 `approval_token`，要求 `--execute`、`PURGE <库名>` 逐字确认和不可覆盖回执；只删除政策 v2 的运行态行与普通过期诊断文件，有效/永久会话、未解决失败、写入未知诊断与审计/资金/业务事实永不删除。每批独立事务，批准计数是硬上限，漂移候选留待重新演练；无论成功或失败都会追加 `auction.retention.purge` 审计。执行前仍须由用户批准候选计数，且建议先在隔离克隆实例演练。
