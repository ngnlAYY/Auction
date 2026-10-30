# 备份与恢复手册

[文档导航](docs/README.md) · [运维手册](ops/README.md) · [auction-cli](apps/auction-cli/README.md) · [数据库契约](docs/contracts/database.md)

本手册描述 PostgreSQL 数据的备份、校验、恢复与破坏性重建。命令参数的完整说明见 [auction-cli](apps/auction-cli/README.md#备份与恢复)。所有写数据库的步骤都要求先停止三个角色（停机门禁），并在维护窗口内执行。

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

同时确认没有其他客户端（报表、手工 psql 会话）连接目标库，并阻止它们自动重连。

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
- 单事务执行：失败时数据库保持为空（可修正后重试）；提交后若后续核对失败，库已非空，不能当作空库重试，须重新准备空库。
- 备份的 schema 版本不得高于当前 `auction-cli`；恢复不保留对象 owner 与权限（空库归应用角色所有，恢复后的对象也归它所有）。

### 5. 迁移与启动

```bash
auction-cli --config-dir /etc/auction db status        # 备份版本较旧时显示未到当前版本
systemctl start auction-stack.service                  # service start 会先迁移
```

合并部署：以服务账号执行 `auction-cli --config-dir /var/lib/auction/config db migrate`，再启动 `auction-server`。

### 6. 恢复后核对

数据库回到了备份时刻，但外部世界没有：备份之后 Bot 可能已经寄出过邮件、收取过附件或发过帖子。恢复后的数据库会认为这些动作尚未发生或仍在进行中。启动后请：

- 在 Admin “投递队列”与“结算报表”中检查进行中与结果未知的投递/履约——系统对处于“发送中”的投递只做 Sent 证据核对、不会盲目重发，但在备份时刻尚未开始的投递会被正常发送；对照 Bot 账号的 Sent 记录确认没有重复寄出。
- 检查邮件轮询游标：备份之后已处理的入站邮件会被重新读取，系统按“邮箱 + 远端邮件 ID”去重，但前提是这些记录在备份中——备份之后接受的投稿需要人工核对附件是否已被收取。
- 检查论坛主题（`auction-cli forum recovery`）与论坛私信（`auction-cli pm list`）。
- 必要时先暂停收件轮询与相关循环，核对完成后再恢复。

## 演练

定期在**隔离的 PostgreSQL 实例**上演练完整恢复。使用同名数据库（manifest 绑定库名摘要，不能恢复到其他库名）。一次性演练库可用 `db rebuild --ephemeral --confirm 'EPHEMERAL <库名>'` 代替备份前提快速重建，但 `--ephemeral` 只能用于可丢弃的库，`empty-for-restore` 模式不接受它。

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
| `mutation_state` | 执行结果：未变更、已完成，或失败/未知阶段 |
| `database_dropped` / `database_recreated` | `true`/`false` 为确定结果；`null` 为无法确认 |

出现失败或未知时：保持全部角色停止，用 `db preflight` 与 `psql` 核对目标库是否存在、是否为空、OID 是否变化，再决定是重新准备空库还是直接恢复。命令进程若在 DROP 之后被强制终止，可能来不及写回执，此时以数据库 catalog 与备份为最终依据。

## 数据保留

`auction-cli db inventory` 按[保留政策](ops/deploy/retention-policy.v1.json)统计各数据族的规模，只读、不授权任何归档或删除。当前政策对业务数据全部为永久保留；任何清理都需要先由业务负责人确定保留期限、法律保留与归档方案，再单独实现与评审。
