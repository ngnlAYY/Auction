# canonical PostgreSQL v28 接管手册

[文档导航](docs/README.md) · [格式与 bridge](crates/auction-storage/src/database/legacy_v28) · [备份与接管编排](apps/auctionctl/src/bootstrap/postgres_legacy_v28) · [当前格式恢复](ops/deploy/RECOVERY.md)

只适用于本仓库 legacy_v28 适配器定义的 canonical PostgreSQL migration 28 历史格式。普通 migrate 对无 SQLx 标记且已有用户
对象的库失败关闭；禁止跳过保护或伪造 `_sqlx_migrations`。

## 本页导航

- [使用范围与结果判读](#使用范围与结果判读)
- [接管前提](#接管前提)
- [1. 停止角色并只读预检](#1-停止角色并只读预检)
- [2. 创建并离线校验专用恢复点](#2-创建并离线校验专用恢复点)
- [3. 执行接管](#3-执行接管)
- [4. 导入数据库之外的规则](#4-导入数据库之外的规则)
- [5. 失败处理](#5-失败处理)
- [6. 启动门槛](#6-启动门槛)

## 使用范围与结果判读

命令只供获准维护窗口执行；库名、路径和日期须替换为精确目标，证据不可覆盖。进程退出或文件存在不能单独证明数据库操作成功。

| 阶段 | 成功依据 | 不能替代 |
| --- | --- | --- |
| 专用备份 | v28 manifest、哈希和精确格式预检 | 当前 Rust backup manifest |
| 接管 | bridge、前向迁移、postflight 和正常 receipt | 五语言规则目录导入 |
| 规则导入 | 五语言 imported/preserved 和去 BOM 正文哈希 | 数据库之外未保管的来源内容 |
| 提交后异常 | 失败记录、审计事实、前向迁移与新恢复点组成替代证据包 | 伪造正常 receipt 或再次执行 adoption |

## 接管前提

- PostgreSQL 必须是 18.x 的可写主节点，数据库名必须与 Worker 敏感 TOML 的目标完全一致。
- `public.schema_migrations` 必须且只能有 `version=28, dirty=false`。
- Public、Admin、Worker、来源服务、后台脚本和人工 SQL 客户端必须全部停止。
- archive、manifest 和 receipt 的父目录必须预先创建，只允许运行账号访问，目标文件不得存在。
- 停止来源服务前必须从其有效配置确认 `rules_directory`；把五个语言文件逐字复制为私有普通文件并保存原文件及移除单个开头
  UTF-8 BOM 后正文的 SHA-256，不能假设 rules_documents 含有当前展示规则。
- 操作员必须保留 v28 专用 archive/manifest，直到切换和恢复窗口正式结束。

只读预检还会核对 canonical public 表集合、schema identity、旧约束/索引/追加式 bid 触发器，并证明每个 policy、lot 领先投影
和原始 bid realtime 截止事实都能确定性转换；任一不匹配或无法转换计数不为零都表示源库不是受支持形状，不能靠手工删列、
改标记或跳过检查继续。

## 1. 停止角色并只读预检

按实际部署方式停止 Rust 角色、所有来源进程和数据库客户端。以下为分体总控示例：

```bash
sudo /opt/auction/bin/auctionctl deployment stop

AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl \
    postgres-adopt-legacy-v28 \
    --confirm-database auction_production
```

命令默认只读。只有输出 `dry-run passed` 且 `backup_verified=false` 时才进入备份步骤；它不会创建 SQLx 历史、修改来源表或
发布 receipt。

## 2. 创建并离线校验专用恢复点

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl \
    postgres-legacy-v28-backup \
    --confirm-database auction_production \
    --output /secure/auction-backups/auction-legacy-v28-2026-08-23.dump

AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl \
    postgres-legacy-v28-backup-verify \
    --confirm-database auction_production \
    --manifest /secure/auction-backups/auction-legacy-v28-2026-08-23.dump.manifest
```

备份命令在 `pg_dump --format=custom` 前后重复 v28 格式预检，校验 `pg_restore --list`，并把 archive SHA-256、数据库名摘要、
migration 28 和稳定预检指纹写入不可覆盖的 0600 manifest；密码只经短生命周期 `.pgpass` 传递。专用 manifest 格式为
`auction-postgres-legacy-v28-backup.v1`，普通 `postgres-restore` 会拒绝它。随后带 manifest 再做一次只读演练，证明 live 源形状
仍与恢复点一致：

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl \
    postgres-adopt-legacy-v28 \
    --confirm-database auction_production \
    --backup-manifest /secure/auction-backups/auction-legacy-v28-2026-08-23.dump.manifest
```

## 3. 执行接管

receipt 必须使用新的绝对路径；命令会在数据库 mutation 前再次验证 archive、manifest 和 live 预检指纹，防止预检期间替换证据。

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl \
    postgres-adopt-legacy-v28 \
    --confirm-database auction_production \
    --backup-manifest /secure/auction-backups/auction-legacy-v28-2026-08-23.dump.manifest \
    --execute \
    --confirm-adoption 'ADOPT LEGACY V28 auction_production' \
    --receipt /secure/auction-evidence/adopt-v28-2026-08-23.json
```

持有 SQLx migration advisory lock 后，在同一物理连接重新预检，避免自有池连接误计为外部 session；随后以 Serializable 事务和
关键表独占锁执行转换：

- 从冻结 policy 和论坛同步投影补齐 Rust 生命周期读取列；未开放拍卖的结束时间继续为空。
- 把 v28“无领先者时 `current_price=start_price`”转换为当前 schema 的空当前价，并从最后一条 bid 恢复 `leader_proxy_last`；
  v28 没有 `material` 事实，兼容值固定为 `false`。
- 从每条原始 `lot.bid_accepted` realtime 事件恢复对应 bid 的当次 `effective_end_at`；追加式 bid 触发器只在独占锁事务内
  短暂关闭，回填后立即恢复。
- 保留旧 final-fast transition 证据，只把可变代理授权投影升级为显式 `final_fast`，并补齐兼容历史表、只读 `bid_proxies`
  视图及当前查询索引。
- 为等价覆盖的 SQLx `1..=5`、`7` 写入当前 checksum，再执行 `6`、`8` 及后续迁移。后续迁移校验 draft/target 顺序与身份、
  移除论坛自动入队 trigger、归一化告警重试和 cooldown；无 delivery 的旧 pending/claimed 告警批次终结为 retry_exhausted，
  历史终态孤儿保留，约束保护所有新写入。完整 SQL 见[迁移树](migrations/postgres/README.md)。

成功后命令重新读取 schema identity 和完整 SQLx 历史，只有两者均为当前版本才发布
`auction-postgres-legacy-v28-adoption.v1` receipt，内含数据库名摘要、备份/manifest 哈希、基线版本、最终 SQLx 版本和数据库
审计 ID，不含连接串、Cookie 或密钥。

## 4. 导入数据库之外的规则

canonical v28 允许权威规则只存在于 `rules_directory/{locale}.md`，`rules_documents` 可能为空，因此 adoption receipt 只证明
数据库接管成功。保持全部角色停止，使用指向同一已接管数据库的 Admin 配置执行：

```bash
AUCTION_CONFIG_FILE=/etc/auction/admin.toml \
  /opt/auction/bin/auctionctl \
    import-rules-directory \
    --directory /secure/auction-migration/rules \
    --command-id 123e4567-e89b-42d3-a456-426614174000 \
    --actor legacy-rules-import
```

目录必须包含 `en.md`、`zh-CN.md`、`zh-TW.md`、`ja.md` 和 `ko.md` 五个 UTF-8 普通文件。命令先完整读取五个来源，再读取五种
数据库文档；任一现有文档的标题或正文不同都会在首次写入前阻断，相同文档报告 `preserved`，只有缺失文档才通过 Admin rules
的 CAS、幂等命令账本和追加审计发布。JSON 的 sha256 对应移除单个开头 UTF-8 BOM 后的完整 Markdown（标题仍保留于正文），
须与预先保存的去 BOM 哈希一致，无 BOM 时才等于原文件哈希；首个非空行去前导 # 后用于标题。部分发布后因依赖失败时，以同一
command-id 重试，不改正文或直接改表绕过冲突。

有效 Admin 配置选择 Redis 时，制品还必须启用 `cache-redis` feature，此时正常启动门槛要求 `cache_invalidation_failures`
为空；若失效失败，应先按缓存运行手册清除对应 `auction:read-model:public-rules:v1:<locale>` 可丢弃键，再继续。
缓存失败不回滚已经提交的权威规则。

## 5. 失败处理

在 bridge 事务提交前失败时，数据库转换会回滚，receipt 不会发布；保持所有角色停止，保存错误输出，修复外部前提后重新执行
只读预检，不要修改来源迁移或绕过不匹配计数。

若错误写明“bridge committed, but a later forward migration failed”，旧语义转换和 SQLx 基线已经提交；若写明
“adoption committed but postflight/receipt failed”，bridge 和前向迁移也已提交，只是数据库与文件系统之间无法原子发布正常
receipt。两种情况都不得再次运行接管命令，也不得启动任何角色，先使用同一二进制和配置继续或复核普通前向迁移：

```bash
AUCTION_CONFIG_FILE=/etc/auction/public.toml \
  /opt/auction/bin/auctionctl migrate
```

迁移成功后运行 `recovery-preflight`，保存当前 schema identity、完整 SQLx 历史、数据库中的 `schema.legacy_v28_adopted` 审计
事实和原失败记录，并立即使用普通 `postgres-backup` 建立当前 Rust schema 恢复点；该异常路径不会补造成功 receipt，
postflight 本身失败时先修复只读盘点条件并重新收集，不得覆盖数据库或把空 receipt 当作成功。

如果决定恢复到来源 v28 格式，先离线运行 `postgres-legacy-v28-backup-verify`，保持全部角色停止，由数据库管理员显式准备同名、
空的 PostgreSQL 18 数据库，再以运行角色执行 `pg_restore --exit-on-error --single-transaction --no-owner --no-privileges`。
这是独立的破坏性恢复决策，不由 adoption 或普通 `postgres-restore` 自动执行；恢复后必须重新运行本手册第 1、2 步证明得到
的仍是 canonical v28，不得在已部分接管的数据库上覆盖恢复。

## 6. 启动门槛

正常路径只有以下证据同时成立后才能运行部署启动总控：

- adoption 命令正常退出并发布新的不可覆盖 receipt。
- receipt 的 archive/manifest SHA-256 与保管的恢复点一致。
- 普通 `auctionctl migrate` 已无待执行迁移。
- 规则导入覆盖五语言 imported/preserved，正文 SHA-256 与来源目录的去 BOM 证据一致。
- Public、Admin、Worker 的目标配置仍指向同一已接管数据库。

若进入“bridge 已提交、后续 migration 失败”或“接管已提交、postflight/receipt 失败”的异常路径，则以原 v28 manifest、失败
记录、recovery preflight、数据库中的 `schema.legacy_v28_adopted` 审计 ID、成功完成的普通 migration 证据和新建的当前 Rust
`postgres-backup` manifest 组成替代证据包，并由部署负责人明确批准后再启动；不得伪造正常接管 receipt。

真实来源数据的业务抽样、外部 MoogleMail/论坛回执和切换前行为验证属于后续显式验收范围。
