# PostgreSQL 备份与恢复

本文冻结 Auction Rust 仓库拥有的 PostgreSQL 备份、显式空库重建、恢复和证据边界。它不替代
部署平台的维护窗口、进程停止、异地备份、加密、保留期限和恢复演练。

## 安全边界

- `auctionctl postgres-restore` 只连接 Worker TOML 引用的 PostgreSQL，不接受命令行连接串。
- `auctionctl postgres-rebuild` 默认只读；只有 `--execute`、双重目标确认、恢复证据和私有 receipt
  同时成立时才拥有 `DROP DATABASE`/`CREATE DATABASE`。
- `--confirm-database` 必须与连接实际返回的 `current_database()` 和 manifest 数据库摘要同时匹配。
- restore 不执行 `DROP DATABASE`、`CREATE DATABASE`、schema 清理或连接终止，也不会覆盖非空库。
- Public、Admin、Worker 和其他数据库客户端必须由部署 owner 在维护窗口外部停止，并阻止自动重连。
- archive、manifest、preflight 和 receipt 使用规范化绝对路径；父目录必须已存在且不得允许
  group/other 写入，文件不得允许 group/other 读取。
- 文件内容和父目录均完成同步后，archive、manifest 和 receipt 才能报告发布成功；跨目录发布同步
  源、目标两个目录。恢复、重建及 v28 接管在数据库 mutation 前也同步用户提供的既有恢复证据。
  不支持目录同步的平台或文件系统失败关闭；仅成功读取或匹配 SHA-256 不足以证明断电后的可恢复性。
- 数据库密码只通过短生命周期的 0600 `.pgpass` 交给 PostgreSQL 客户端，不进入参数、receipt 或日志。
- 备份/恢复的敏感 `database_url` 必须显式提供 host 和 port，默认端口也写为 `5432`；外部工具
  清除继承的 `PGHOST`、`PGHOSTADDR`、`PGPORT`，仅使用该 URL 的目标，避免与 SQLx 预检分歧。

## 1. 停止流量并创建备份

先由服务管理器停止 Public、Admin、Worker 和所有其他客户端。备份目录与证据目录应由部署账号
独占，例如：

```bash
install -d -m 0700 /secure/auction-backups /secure/auction-evidence

AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl postgres-backup \
    --confirm-database auction_production \
    --output /secure/auction-backups/auction-2026-08-23.dump

/opt/auction/bin/auctionctl postgres-backup-verify \
  --confirm-database auction_production \
  --manifest /secure/auction-backups/auction-2026-08-23.dump.manifest
```

`.dump` 和 `.dump.manifest` 必须作为同一个恢复点一起复制到部署方拥有的异地存储。manifest
包含数据库名摘要、archive SHA-256 和 SQLx schema 版本，但没有数字签名；目录访问控制、异地
不可变存储和独立演练仍是部署责任。

## 2. 准备空目标

先执行默认只读的完整重建预检；不带 `--execute` 时不会终止连接、删除或创建数据库：

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl postgres-rebuild \
    --confirm-database auction_production \
    --mode empty-for-restore \
    --backup-manifest /secure/auction-backups/auction-2026-08-23.dump.manifest
```

预检要求目标仍是当前且 clean 的 Auction schema，并检查 PostgreSQL 18 主节点、零外部 session、
零 prepared transaction、零 subscription/活动 logical slot、目标 owner/`CREATEDB` 权限、locale、
tablespace、自定义 database ACL 和 database/role setting。自定义 tablespace 要求维护角色具有显式
`CREATE` 权限；`pg_default` 使用 PostgreSQL 允许 `CREATEDB` owner 采用的隐式默认路径，不要求为了
重建而额外授予全局 tablespace 权限。维护连接从 Worker `database_url` 派生到
同集群 `postgres` 数据库；生产环境推荐改用 owner-only 的独立 `--maintenance-secrets-file`，其
高权限 URL 必须连接同 endpoint/TLS profile 的 `postgres` 数据库，且不会进入命令行、日志或
receipt。应用和维护 URL 均须显式写出单一 host、非零 port（包括默认 5432）和数据库名；端点
由实际 SQLx 解析结果比较，不能依赖 `PGHOST`/`PGPORT` 的环境默认值。IPv6 字面量在 URI 中保留
方括号，实际 SQLx TCP host、libpq 的 `PGHOST` 和临时密码文件统一使用去括号的地址；密码文件
继续转义冒号。查询参数仅接受
`application_name`、`sslmode`；请用 `sslmode` 替代 SQLx 忽略的 `ssl=true`，并移除其同样忽略的
`connect_timeout`。两边 `sslmode` 必须相同或同时省略；未配置独立维护文件时只替换数据库名。自定义 ACL/setting 会失败关闭，因为删库后静默丢失这些安全策略比拒绝自动化更危险。

预检通过后，实际准备同名空库必须使用新的 receipt 路径和逐字确认短语：

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl postgres-rebuild \
    --confirm-database auction_production \
    --mode empty-for-restore \
    --backup-manifest /secure/auction-backups/auction-2026-08-23.dump.manifest \
    --maintenance-secrets-file /etc/auction/postgres-rebuild.secret.toml \
    --execute \
    --confirm-rebuild 'REBUILD auction_production' \
    --receipt /secure/auction-evidence/rebuild-2026-08-23.json
```

命令在取得集群 advisory lock 并二次校验 archive/manifest 后执行
`DROP DATABASE ... WITH (FORCE)`，再以 `template0`、原 owner、UTF8、locale provider、tablespace
和 connection limit 重建，并重新连接证明目标为空。目标为 `pg_default` 时 DDL 省略显式
`TABLESPACE` 子句，以符合普通 `CREATEDB` owner 的 PostgreSQL 权限语义；非默认 tablespace 仍逐字
保留并要求前述权限。`empty-for-restore` 强制要求 backup manifest，
不能使用 unrecoverable ephemeral 豁免。不要先运行 `auctionctl migrate`：恢复目标必须没有 Auction
migration 表，也不能存在用户 schema、类型、关系、函数、扩展、publication、subscription、
large object 或 default ACL。

新回执格式为 `auction-postgres-rebuild.v2`；历史 `auction-postgres-rebuild.v1` 不改写。v2 的 `database_dropped`、
`database_recreated` 和 `forced_drop` 可为 `true`、`false` 或 `null`，`null` 表示未取得完成证明，
不能按未发生处理。若状态为 `mutation_unknown`、`mutation_failed` 或 `postcondition_failed`，
保持全部角色停止，按上述状态和 `failure_stage` 人工核对数据库 catalog/OID 与备份后恢复；
不要盲目再次 DROP。DROP/CREATE 响应丢失时也会尝试发布未知结果回执，留证失败会附加到原错误链。
命令失败不等于原库仍然存在。进程在 DROP 后被强制终止等极端情况可能来不及发布回执，
因此数据库 catalog 和备份仍是最终证据。

恢复到不同数据库名会因 manifest 身份不匹配而失败。需要恢复演练时，应在隔离 PostgreSQL 实例中
使用同名数据库，而不是放宽该确认条件。

## 3. 只读预检与 dry-run

独立保存空库预检证据：

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl recovery-preflight \
    --confirm-database auction_production \
    --output /secure/auction-evidence/recovery-preflight-2026-08-23.json
```

随后把 archive 校验和目标预检合并执行，但不写数据库：

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl postgres-restore \
    --confirm-database auction_production \
    --manifest /secure/auction-backups/auction-2026-08-23.dump.manifest \
    --dry-run
```

dry-run 通过只证明该时刻 archive、manifest、数据库身份、可写状态、活动连接和空库条件满足。
它不会预留或写入 receipt，也不能阻止外部客户端在检查后重新连接。

## 4. 单事务恢复

实际恢复必须提供一个尚不存在的 receipt 路径：

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl postgres-restore \
    --confirm-database auction_production \
    --manifest /secure/auction-backups/auction-2026-08-23.dump.manifest \
    --receipt /secure/auction-evidence/restore-2026-08-23.json
```

命令会先安全预留 receipt 临时文件，然后执行以下顺序：

1. 验证 manifest 格式、数据库摘要、archive 文件名、SHA-256 和 `pg_restore --list`。
2. 在只读事务中要求目标为可写主节点、没有其他连接、没有任何用户对象。
3. 关闭工具自身 SQLx 连接池，再次验证 archive 与 manifest 未变化。
4. 执行 `pg_restore --exit-on-error --single-transaction --no-owner --no-privileges`。
5. 重新连接并核对 Auction schema identity、clean SQLx migration 历史和 manifest schema 版本。
6. 以不可覆盖方式发布 `auction-postgres-restore.v1` JSON receipt。

`pg_restore` 返回失败时，其单事务应整体回滚，命令不会发布 receipt。若恢复已经提交但恢复后
schema 校验或 receipt 发布失败，数据库不再是空库；保持全部角色停止，保留诊断和临时证据，重新
准备空库后再执行新的恢复，不要把失败运行误报为成功。

## 5. 前向迁移与重新放量

旧 archive 的 schema 版本可以低于当前二进制，但不能高于当前内嵌迁移版本。恢复成功后先执行正常
前向迁移，再检查 readiness：

```bash
AUCTION_CONFIG_FILE=/etc/auction/public.toml \
  /opt/auction/bin/auctionctl migrate
```

随后按 Worker → Admin → Public 顺序启动，并检查两个 HTTP 进程的 `/health/live` 和
`/health/ready`。保留 archive、manifest、preflight、restore receipt、迁移输出、操作员、维护窗口和
最终 readiness 结果。任何切换连接、回收恢复点或 retention mutation 都需要独立授权；
`postgres-rebuild` 的授权只覆盖命令中精确确认的同名数据库和该次不可覆盖回执。
