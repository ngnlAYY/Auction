# Auction-BF PostgreSQL v28 接管手册

本手册只适用于旧版 Auction-BF 最终的 canonical PostgreSQL migration 28。普通
`auctionctl migrate` 遇到没有 SQLx 标记但已有用户对象的数据库会失败关闭；不得绕过保护，
也不得手工伪造 `_sqlx_migrations`。

## 接管前提

- PostgreSQL 必须是 18.x 的可写主节点，数据库名必须与 Worker 敏感 TOML 的目标完全一致；
- `public.schema_migrations` 必须且只能有 `version=28, dirty=false`；
- Public、Admin、Worker、旧版服务、后台脚本和人工 SQL 客户端必须全部停止；
- archive、manifest 和 receipt 的父目录必须预先创建，只允许运行账号访问，目标文件不得存在；
- 操作员必须保留旧版 archive/manifest，直到切换和恢复窗口正式结束。

只读预检还会核对 canonical public 表集合、schema identity、旧约束/索引/追加式 bid 触发器，
并证明每个 policy、lot 领先投影和原始 bid realtime 截止事实都能确定性转换。任一计数不为零都
表示源库不是受支持形状，不能靠手工删列、改标记或跳过检查继续。

## 1. 停止角色并只读预检

先通过部署总控停止 Rust 三角色，并停止所有旧版进程和数据库客户端：

```bash
sudo /opt/auction/bin/auctionctl deployment stop

AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl \
    postgres-adopt-legacy-v28 \
    --confirm-database auction_production
```

命令默认只读。只有输出 `dry-run passed` 且 `backup_verified=false` 时才进入备份步骤；它不会
创建 SQLx 历史、修改旧表或发布 receipt。

## 2. 创建并离线校验专用恢复点

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl \
    postgres-legacy-v28-backup \
    --confirm-database auction_production \
    --output /secure/auction-backups/auction-bf-v28-2026-08-23.dump

AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl \
    postgres-legacy-v28-backup-verify \
    --confirm-database auction_production \
    --manifest /secure/auction-backups/auction-bf-v28-2026-08-23.dump.manifest
```

备份命令在 `pg_dump --format=custom` 前后重复旧库预检，校验 `pg_restore --list`，并把 archive
SHA-256、数据库名摘要、migration 28 和稳定预检指纹写入不可覆盖的 0600 manifest。密码只经
短生命周期 `.pgpass` 传递。专用 manifest 格式为
`auction-postgres-legacy-v28-backup.v1`，普通 Rust `postgres-restore` 会拒绝它。

随后带 manifest 再做一次只读演练，证明 live 源形状仍与恢复点一致：

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl \
    postgres-adopt-legacy-v28 \
    --confirm-database auction_production \
    --backup-manifest /secure/auction-backups/auction-bf-v28-2026-08-23.dump.manifest
```

## 3. 执行接管

receipt 必须使用新的绝对路径；命令会在数据库 mutation 前再次验证 archive、manifest 和 live
预检指纹，防止预检期间替换证据。

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl \
    postgres-adopt-legacy-v28 \
    --confirm-database auction_production \
    --backup-manifest /secure/auction-backups/auction-bf-v28-2026-08-23.dump.manifest \
    --execute \
    --confirm-adoption 'ADOPT LEGACY V28 auction_production' \
    --receipt /secure/auction-evidence/adopt-v28-2026-08-23.json
```

接管在 SQLx migration advisory lock、Serializable 事务和关键表独占锁内完成以下转换：

- 从冻结 policy 和论坛同步投影补齐 Rust 生命周期读取列；未开放拍卖的结束时间继续为空；
- 把旧版“无领先者时 `current_price=start_price`”转换为 Rust 的空当前价，并从最后一条 bid 恢复
  `leader_proxy_last`；旧版没有 `material` 事实，因此兼容值固定为 `false`；
- 从每条原始 `lot.bid_accepted` realtime 事件恢复对应 bid 的当次 `effective_end_at`；追加式 bid
  触发器只在独占锁事务内短暂关闭，回填后立即恢复；
- 保留旧 final-fast transition 证据，只把可变代理授权投影升级为显式 `final_fast`，并补齐兼容
  历史表、只读 `bid_proxies` 视图及当前查询索引；
- 为已经等价覆盖的 SQLx `1..=5`、`7` 写入当前二进制内嵌 checksum，再依次运行 `6`、`8` 及
  后续所有尚未应用的前向迁移；`000014` 会先核对并补齐可唯一恢复的 draft target，再重建
  拍卖身份、前序顺序和 draft/target 一致性防线，同时移除旧版论坛自动入队 trigger，使论坛
  任务只由 Worker 发现和领取；`000015` 会归一化 operations alert 的错误码、重试终态和 signal
  cooldown。旧版 Create 前形成且没有 delivery 的 pending/claimed 批次会安全终结为
  `retry_exhausted`；历史终态孤儿继续保留，外键会约束所有迁移后新写入，并在不存在历史孤儿时
  完成全表验证。

成功后命令重新读取 schema identity 和完整 SQLx 历史，只有两者均为当前版本才发布
`auction-postgres-legacy-v28-adoption.v1` receipt。回执包含数据库名摘要、备份/manifest 哈希、
基线版本、最终 SQLx 版本和数据库审计 ID，不包含连接串、Cookie 或密钥。

## 4. 失败处理

在 bridge 事务提交前失败时，数据库转换会回滚，receipt 不会发布。保持所有角色停止，保存错误
输出，修复外部前提后重新执行只读预检；不要修改旧迁移或绕过不匹配计数。

如果错误明确写明“bridge committed, but a later forward migration failed”，旧语义转换和 SQLx
基线已经提交。此时不得再次运行接管命令，也不得启动任何角色；使用同一二进制和配置继续普通
前向迁移：

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl migrate
```

迁移成功后保存数据库中的 `schema.legacy_v28_adopted` 审计事实和原失败记录，并立即使用普通
`postgres-backup` 建立当前 Rust schema 恢复点。该异常路径不会补造一次原接管成功 receipt。

如果决定恢复到旧版 v28，先离线运行 `postgres-legacy-v28-backup-verify`，保持全部角色停止，由
数据库管理员显式准备同名、空的 PostgreSQL 18 数据库，再以运行角色执行
`pg_restore --exit-on-error --single-transaction --no-owner --no-privileges`。这是独立的破坏性恢复
决策，不由 adoption 或普通 `postgres-restore` 自动执行。恢复后必须重新运行本手册第 1、2 步，
证明得到的仍是 canonical v28；不得在已部分接管的数据库上覆盖恢复。

## 5. 启动门槛

正常路径只有以下证据同时成立后才能运行部署启动总控：

- adoption 命令正常退出并发布新的不可覆盖 receipt；
- receipt 的 archive/manifest SHA-256 与保管的恢复点一致；
- 普通 `auctionctl migrate` 已无待执行迁移；
- Public、Admin、Worker 的目标配置仍指向同一已接管数据库。

若进入“bridge 已提交、后续 migration 失败”的异常路径，则以原 v28 manifest、失败记录、数据库
中的 `schema.legacy_v28_adopted` 审计 ID、成功完成的普通 migration 证据和新建的当前 Rust
`postgres-backup` manifest 组成替代证据包，并由部署负责人明确批准后再启动；不得伪造正常接管
receipt。

真实旧数据的业务抽样、外部 MoogleMail/论坛回执和切换前行为对照属于后续显式验收范围；按项目
约束，未经用户要求不在本手册中编写或运行测试用例及浏览器测试。
