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

演练会持锁检查：维护库为 `postgres`、PostgreSQL 18 可写主节点、目标允许连接且非模板、无自定义 ACL 或库级设置、UTF8 编码、维护角色为 superuser 或“目标 owner + CREATEDB”、零活动会话、无预备事务、无活动逻辑复制槽与订阅；并再次校验备份。通过后执行：

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
