# 迁移演练

[ops/README.md](../README.md) · [ops/deploy/RECOVERY.md](RECOVERY.md) · [apps/auction-cli/README.md](../../apps/auction-cli/README.md) · [ops/deploy/migration-drill-receipt.template.json](migration-drill-receipt.template.json)

含迁移的候选版本在部署前，必须用**最新可验证备份、同一候选制品和隔离 PostgreSQL 18**完成恢复 → 迁移 → 三角色 readiness → 一致性巡检，记录耗时与结论。本流程只提供手工步骤与回执，不新增演练命令，不自动放行部署，也不表示已经完成过真实演练。本流程单独成文以便随制品留存升级证据；恢复原理和破坏性操作规则仍由 `ops/deploy/RECOVERY.md` 维护。

职责边界：本页拥有演练步骤、回执字段取值与审核标准；`auction-cli deploy` 对回执的机器核对在[apps/auction-cli/README.md](../../apps/auction-cli/README.md)与代码中定义；恢复/重建/回执判读由[ops/deploy/RECOVERY.md](RECOVERY.md)拥有；schema 与兼容等级由[docs/contracts/database.md](../../docs/contracts/database.md)拥有。三处不得各自定义一套判定。

## 准备与隔离

- 由执行人获准后操作本机或隔离实例；**不能连接生产数据库、复用生产 unit/socket/缓存目录，或向游戏、论坛、R2、打卡与 tracing 服务发请求**。主机/容器网络只放行隔离 PG、独立缓存及本机探针；禁用邮件/论坛仅是第二道边界，恢复库中仍有真实凭据和历史待办。
- 目标必须 PG18 独立实例/端口，示例为 `127.0.0.1:55432`；数据库名仍为 `auction`（manifest 绑定原库名摘要），若源库不同，逐处替换。不能用生产实例上的另一个库冒充隔离，也不能修改 manifest 绕过库名绑定。先人工核对实例身份、监听端口、磁盘余量与其他连接。
- 把 CI 候选制品解压到 `/srv/auction-drill/candidate/`，校验包摘要和包内 `SHA256SUMS`，记录版本、完整提交、包摘要与新 CLI 摘要。保留当前线上版本的旧制品于 `/srv/auction-drill/previous/`，用于测量停机排空。演练必须使用待部署制品；任何迁移、二进制或相关配置变更后重做并生成新回执。
- 创建独立 `/srv/auction-drill/config/`，三角色使用同一个隔离 `common_file`，runtime 要么三者同用隔离副本，要么三者都省略。敏感 TOML 中的 PG 与维护连接都指向隔离端口；维护库为 `postgres`，应用库为原库名；保存备份所需的历史数据密钥，敏感文件 0600，不把连接串、Cookie、密钥、玩家信息或 TOML 原文写进回执。
- 在有效配置中关闭 `[worker.mooglemail]`、`[worker.forum]`、`[worker.operations_alert]` 和 `[auction.schedule]` 的 `enabled`；runtime 若接管 schedule，也须关闭其 `operations.schedule.enabled`。关闭 tracing/异机备份，打卡地址留空。`auction.schedule.enabled=false` 只停止新排期创建，Worker 仍可处理恢复库中的既有生命周期/结算，所有变化仅允许进入隔离副本。
- Public/Admin 仅监听本机独立端口（示例 `18090/18091`），Public 使用独立 realtime consumer；Worker control 启用，socket 使用 `/srv/auction-drill/run/worker/control.sock`，目录、组和 peer allowlist 按[ops/config/README.md#worker](../config/README.md#worker)准备。Admin 的 worker control 路径与之匹配。用新 CLI 的 `config check --strict` 和 `config show worker` 核对有效配置，不能只修改被 runtime 覆盖的 fallback。
- 示例使用直接运行的分体角色，始终不装入生产 systemd；因此数据库写命令显式 `--no-systemd`，由执行人确认本演练三角色均已停止。此声明不跳过 `auction-server` 的 owner 锁。合并部署改用同一候选提交的合并制品、独立配置运行 supervisor，记录其实际排空时间，停止后以同一服务账号通过 owner 锁迁移；readiness 仍逐角色检查。

以下命令是由获准执行人运行的手工步骤，不是开发阶段的默认验证。所有输出写入同一个新的私有证据目录；各阶段检查退出码，非零或结果未知即停止后续步骤。

```bash
umask 077
drill_cli=/srv/auction-drill/candidate/bin/auction-cli
drill_config=/srv/auction-drill/config
drill_evidence=/srv/auction-drill/evidence/drill-001
# 每次演练改用尚不存在的目录；备份文件及清单须成对放在此私有目录
install -d -m 0700 "$drill_evidence"
"$drill_cli" --config-dir "$drill_config" config check --strict
"$drill_cli" --config-dir "$drill_config" config show worker
```

隔离配置同样遵守账号分离：三角色 `secrets.toml` 的 `[database].username` 是运行账号 `auction_runtime`，`common.toml` 的 `[database_roles] owner` 是迁移 owner；所有需要 DDL 的步骤都用 `--owner-secrets` 指向 owner-only 的迁移凭据文件。

## 1. 校验最新备份

从最新成功备份中取回完整 archive 与 manifest，保持 manifest 内的 archive 文件名和二者同目录。异机加密备份先按[ops/deploy/RECOVERY.md#从-r2-取回并演练恢复](RECOVERY.md#从-r2-取回并演练恢复)核对完成记录、密文长度与摘要、解密认证和明文 manifest 摘要，不在本流程重复定义下载规则。

```bash
"$drill_cli" --config-dir "$drill_config" backup verify --database auction \
  --manifest "$drill_evidence/source.dump.manifest" > "$drill_evidence/backup-verify.json"
sha256sum "$drill_evidence/source.dump" "$drill_evidence/source.dump.manifest"
```

`source.dump` 仅为示例，替换为原 archive 文件名。把备份创建时间、schema 版本、archive/manifest 名称与 SHA-256、verify 结果写入回执；对所有相关备份逐项列清单。`backup verify` 不连数据库，只证明备份字节可验证，不证明其已恢复或可被旧/新程序就绪。新 CLI 不支持高于自身内嵌 schema 的备份。

## 2. 准备空库并恢复

隔离实例尚无同名库时：

```bash
"$drill_cli" --config-dir "$drill_config" db create --database auction \
  --maintenance-secrets /srv/auction-drill/maintenance/postgres.toml \
  --owner-secrets /srv/auction-drill/owner/postgres.toml --no-systemd
```

同名库已存在且需要重置时，先只读预览，核对实例/目标和恢复点，再逐字确认；两条命令都只能指向隔离库：

```bash
"$drill_cli" --config-dir "$drill_config" db rebuild --database auction \
  --maintenance-secrets /srv/auction-drill/maintenance/postgres.toml \
  --owner-secrets /srv/auction-drill/owner/postgres.toml \
  --mode empty-for-restore --backup-manifest "$drill_evidence/source.dump.manifest" \
  --no-systemd > "$drill_evidence/rebuild-preview.json"
"$drill_cli" --config-dir "$drill_config" db rebuild --database auction \
  --maintenance-secrets /srv/auction-drill/maintenance/postgres.toml \
  --owner-secrets /srv/auction-drill/owner/postgres.toml \
  --mode empty-for-restore --backup-manifest "$drill_evidence/source.dump.manifest" \
  --no-systemd --execute --confirm 'REBUILD auction' --receipt "$drill_evidence/rebuild.json"
```

不要在恢复前执行 `db migrate`；恢复必须进入无 schema 的空库。只读预检、dry-run 与真实恢复分别留证：

```bash
"$drill_cli" --config-dir "$drill_config" db preflight --database auction \
  --output "$drill_evidence/empty-preflight.json"
"$drill_cli" --config-dir "$drill_config" backup restore --database auction \
  --owner-secrets /srv/auction-drill/owner/postgres.toml \
  --manifest "$drill_evidence/source.dump.manifest" --dry-run --no-systemd
"$drill_cli" --config-dir "$drill_config" backup restore --database auction \
  --owner-secrets /srv/auction-drill/owner/postgres.toml \
  --manifest "$drill_evidence/source.dump.manifest" --no-systemd \
  --receipt "$drill_evidence/restore.json"
"$drill_cli" --config-dir "$drill_config" db preflight --database auction \
  --output "$drill_evidence/restored-preflight.json"
```

核对恢复回执为成功、schema 等于备份版本，从 preflight 填写 PG 服务端完整版本、实例标签、起始迁移版本。DROP/恢复响应丢失、回执缺失或失败时遵守[ops/deploy/RECOVERY.md#回执状态判读](RECOVERY.md#回执状态判读)，不盲目重复破坏性操作。

**`--ephemeral` 的边界**：既有 `db rebuild --mode fresh-schema --ephemeral` 可对可丢弃隔离库做新 schema 安装检查；实际执行仍需 `--execute --confirm 'EPHEMERAL auction' --receipt <新文件>`。它不恢复旧备份，不能作为本项升级演练的成功证据。`empty-for-restore` 必须使用已验证的 `--backup-manifest`，不接受 `--ephemeral`，不得编造二者组合来省略备份。

## 3. 用新程序迁移并计时

为实测停机时间，先用旧制品在隔离配置下运行三个角色，逐一确认旧版本 readiness。三个终端各以前台 `env AUCTION_CONFIG_FILE=/srv/auction-drill/config/<角色>.toml /srv/auction-drill/previous/bin/<角色程序>` 启动；不向外部发请求。记录第一次向 Public 发出正常 SIGTERM/SIGINT 的 UTC 时间为 `downtime.started_at`，按 Public → Admin → Worker 停止并等待退出，记录排空完成时间。没有可运行的旧制品、备份 schema 不支持旧版本或未测得排空时，本次不能给出完整实测停机时长，回执标为未完成，另行补测。

确认三角色确已停止，再使用**新 CLI**计时迁移；`--no-systemd` 不替执行人证明进程已停：

```bash
date -u +%FT%TZ
/usr/bin/time -p -o "$drill_evidence/migrate-time.txt" \
  "$drill_cli" --config-dir "$drill_config" db migrate \
  --owner-secrets /srv/auction-drill/owner/postgres.toml --no-systemd \
  > "$drill_evidence/migrate.json" 2> "$drill_evidence/migrate.stderr"
date -u +%FT%TZ
"$drill_cli" --config-dir "$drill_config" db status > "$drill_evidence/migrated-status.json"
```

记录迁移 UTC 起止、`time` 的 real 秒数、CLI 退出码，以及 `migration.before_version/after_version/expected_version/applied`。成功要求 after 与 expected 一致，历史前缀和 checksum 均有效。若 `applied=0`，明确记录没有待执行迁移；这只能证明该备份的无迁移路径，不能宣称覆盖了尚未演练的升级区间。不要通过删改迁移历史、改已发布 SQL 或在旧制品上隐式迁移来凑齐版本。

## 4. readiness 与巡检

使用新制品，依次在三个终端以前台启动 Worker、Admin、Public；每个通过 readiness 后才启动下一个。示例启动命令分别为：

```bash
env AUCTION_CONFIG_FILE=/srv/auction-drill/config/worker.toml /srv/auction-drill/candidate/bin/auction-worker
env AUCTION_CONFIG_FILE=/srv/auction-drill/config/admin.toml /srv/auction-drill/candidate/bin/auction-admin-api
env AUCTION_CONFIG_FILE=/srv/auction-drill/config/public.toml /srv/auction-drill/candidate/bin/auction-public-api
```

另一个探针终端按对应阶段运行，期望 HTTP 200；启动未完成可在记录的 readiness 预算内重查，超时记失败，不把一次存活响应当作就绪：

```bash
curl --fail --silent --show-error --max-time 5 \
  --unix-socket /srv/auction-drill/run/worker/control.sock \
  --output "$drill_evidence/worker-ready.json" --write-out '%{http_code}\n' \
  http://localhost/v1/health/ready
curl --fail --silent --show-error --max-time 5 \
  --output "$drill_evidence/admin-ready.json" --write-out '%{http_code}\n' \
  http://127.0.0.1:18091/health/ready
curl --fail --silent --show-error --max-time 5 \
  --output "$drill_evidence/public-ready.json" --write-out '%{http_code}\n' \
  http://127.0.0.1:18090/health/ready
date -u +%FT%TZ
```

保存各角色 PID/制品身份、探针时间、HTTP 状态码、退出码与最终结果；若探测期间角色退出/重启，重查新进程，不能沿用旧进程的 200。最后一个角色通过的时间为 `downtime.ready_at`。readiness 只能证明隔离配置下的新程序就绪，不能证明线上 Cookie、外部发送或资金正确。

一致性巡检已提供正式只读入口 `auction-cli db inspect-consistency`；在 readiness 之后执行，把报告及摘要加入 `consistency_check`，结果必须为 `passed` 并附证据。检查失败、未执行或 `not_available` 一律停止发布；不能捏造命令或填成 passed。必要的人工只读核对另记结果与范围，不冒充完整巡检。

## 停机时长与窗口

`downtime.measured_seconds` 是从旧 Public 停止请求到新 Public/Admin/Worker 全部 readiness 的实测秒数，包含旧角色排空、切换准备、迁移、顺序启动和等待；备份恢复在此之前完成，恢复耗时另记。如果演练没有覆盖生产所需的停机后备份、制品安装或其他步骤，把其测量依据和额外预算逐项记入 `estimate_basis`，令 `estimated_seconds` 不小于实测值并覆盖这些成本。巡检若阻挡恢复业务，也计入停机估计。不要把单独的迁移耗时当作完整停机时长。

运维按 `ceil(3 × max(measured_seconds, estimated_seconds) / 60)` 得到窗口下限，通常保留默认 60 分钟或选更大的值，并在回执记录实际选择：

```bash
auction-cli --config-dir /etc/auction service restart --protection-window-minutes 90
```

`auction-cli service stop/restart` 只验证 1–10080 分钟的范围，不读取演练回执，也不知道三倍下限；`auction-cli deploy` 会对照回执的 `minimum_window_minutes`，`rollback` 只重新执行时段门禁。若所需下限超过范围，先停止该发布方案并重估停机安排，不能靠截断窗口或把 force 当作常规发布流程。更新 systemd 包装命令时同样由运维显式传参；省略参数仍为 60 分钟。

## 回执、结论与后续门禁

复制[ops/deploy/migration-drill-receipt.template.json](migration-drill-receipt.template.json)到新的 0600 工作文件，填写真实值；null/`not_run` 表示未知/未执行，不是通过。记录执行人、日期、隔离身份、候选制品、备份清单/摘要、PG 版本、恢复/迁移/探针证据、各项耗时、异常与结论。对证据文件做 SHA-256 后列入 `evidence_files`，不包含秘密配置或数据库导出正文；长期保存的回执只含安全标识、摘要和结论。审核完成后以新的终态文件封存，不覆盖旧回执；后续纠正形成新回执并引用前次编号。

仅在恢复、实际升级区间、三角色 readiness 及已可用的巡检通过、停机预算可接受、没有未解释异常时，结论为 `passed`；失败、未知或缺证据均暂停发布。演练结束后按 Public → Admin → Worker 正常停止隔离角色，保留备份和全部证据；需要清理库时仍走 `auction-cli db rebuild` 的备份、确认与新回执，不运行手写 DROP 脚本。

`auction-cli deploy` 在停机前自动核对回执并失败关闭；`--drill-max-age-days` 默认 7（1–30），以下任一不满足即拒绝部署，不停止服务、不安装、不迁移：

| 回执字段 | 通过条件 |
| --- | --- |
| `format` / `operator` / `reviewer` / `date_utc` | `auction-migration-drill.v1`；执行人与审核人不同；日期不晚于今天且不超过 `--drill-max-age-days` |
| `artifact` | `commit`、`package`、`cli_sha256` 与本次待部署制品完全一致，`package_sha256` 为 64 位小写十六进制 |
| `backup` | `verify_result = "verified"`、`schema_version` 等于当前数据库版本、`created_at` 非空、`database_name_sha256` 有效；`files` 恰好各一条 `archive`/`manifest` |
| `postgres` | `major = 18`、`isolated = true`、`server_version` 以 `18.` 开头、host/port/database/instance_label 有效 |
| `restore` | `result = "passed"`，含耗时、restore 回执与 preflight 证据 |
| `migration` | `result = "passed"`、`exit_code = 0`、`before_version` 等于当前库、`after_version`/`expected_version` 等于候选制品，`applied` 等于两者之差；含起止时间、耗时与两份证据 |
| `readiness` | worker/admin/public 三项均 `result = "passed"`、`http_status = 200`、`exit_code = 0`、`pid > 0`、有 `checked_at` 与证据；每角色预算为正 |
| `consistency_check` | `work_item = "W11.4"`、`result = "passed"`，有 `evidence` 与 `note`；`not_run` 一律拒绝 |
| `downtime` | 起止/ready 时间齐全且单调；`measured_seconds` 或 `estimated_seconds` 至少一项有效；`minimum_window_minutes ≥ ceil(3 × 秒数 / 60)`，`selected_window_minutes ≥ minimum_window_minutes`，且部署的 `--protection-window-minutes` ≥ 该下限 |
| `evidence_files` / `anomalies` / `conclusion` | 每个证据名为 1–256 字节且 SHA-256 为小写；`anomalies` 必须为空；`conclusion.result = "passed"` |

不同制品或未覆盖的迁移区间不得复用旧回执。手工部署时由执行人和审核人完成同样核对；回执不能通过删改字段或伪造证据“修好”，只能重做演练。

回退：本项不更改 schema；可退回手工步骤与旧版 CLI。真实升级已经提交迁移时，不能因演练通过而直接换旧程序，仍须依据迁移兼容声明或[ops/deploy/RECOVERY.md#恢复流程](RECOVERY.md#恢复流程)及[ops/deploy/RECOVERY.md#6-外部历史核对与启动](RECOVERY.md#6-外部历史核对与启动)决定；结果未知不重发、不改历史审计。
