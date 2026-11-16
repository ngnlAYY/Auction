---
name: auction-production-deploy
description: 更新 Auction 云端分体部署：取 GitHub Release 资产并校验来源、隔离迁移演练与回执、auction-cli deploy、部署后核对与记录
---

# 适用范围

用户明确要求更新 Auction 线上（云端）时使用。授权范围只包括本流程内的部署动作，不包括业务操作、历史数据恢复、Git 推送或打 tag。

权威规则以仓库为准：`ops/README.md#升级`（制品来源、`deploy` 门禁、失败与回退）、`ops/deploy/MIGRATION-DRILL.md`（演练步骤与回执）、`ops/deploy/RECOVERY.md`（备份、恢复与程序回退）。本技能只记录线上与本机的具体事实、可复用工具和踩过的坑；与仓库文档冲突时以仓库为准，并更新本技能。占位符的含义见 `ops/skills/README.md`。

# 制品来源：只部署 GitHub Release 资产

- 只部署 `main` 上 `v<工作区版本>` tag 触发 release 工作流产生的 Release 资产：`auction-rust-<版本>-<短提交>.tar.gz`、`SHA256SUMS`、`DEPENDENCIES.md`。该工作流会重跑全部门禁、`just test`、`just test-db` 与迁移兼容验证，并附 GitHub 来源证明。
- 本地 `just release-check` 产物、普通 CI 候选 artifact 与 `-dirty` 包一律不部署。早期曾用本地包上线，现已停止。
- 打 tag 由维护者执行。目标提交还没有 Release 时，停下来请维护者处理：确认工作区版本（tag 必须等于 `Cargo.toml` 的 `workspace.package.version`，且同一版本不能发布两次），在 `main` 上打 tag 并推送，等 release 工作流完成。代理不打 tag，也不改版本号。

# 线上事实

- 入口：`<生产 SSH 入口>`。生产主机资源很小，**禁止在服务器上构建**。
- 目录按 `ops/README.md#目录与账号` 的标准布局：程序 `<程序目录>`，配置 `<配置目录>`，部署状态 `<部署状态目录>`（`receipts/`、`rollback/`、锁），停机备份 `<停机备份目录>`（root 0700），计划备份 `/var/backups/auction/scheduled/`；发布包解包到 `<发布包暂存目录>/<package>/`。运行账号 `<运行账号>`；systemd unit：`<Worker unit>`、`<Admin unit>`、`<Public unit>`，由 `auction-stack.service` 编排。
- `<程序目录>` 与其中四个程序必须是 root:root 0755，各级父目录也归 root 且禁止他人写入：root 与 postgres 都会执行其中的 `auction-cli`。deploy/rollback 在停机前核对，不符即拒绝、不自动修补；旧版 deploy 曾把程序改归 `<运行账号>`，遇到这种拒绝时按 `ops/deploy/RECOVERY.md#重建安装目录信任` 用已验证的 Release 重装，**不要**只 `chown`，重建属于生产写入，先征得用户同意。
- `<部署状态目录>` 必须归 root 且 0700，各级父目录归 root 且禁止他人写入（`/var/lib/auction` 为 root:root 0755，已只读核实）：能改写父目录的账号就能把部署锁、回执与回滚记录整体换成另一棵树。service/deploy/rollback 取锁前核对，持锁期间被改名替换会在停机前失败关闭，不自动修补；遇到拒绝时先停下报告用户，改属主或权限属于生产写入。
- 配置 `<配置目录>`：`common/public/admin/worker.toml`、`secrets.toml`（运行账号 0600）、`runtime/admin.toml`（0640）、`owner/postgres.toml`（root 0600，只给 CLI 用）。`worker.toml [worker.control] allowed_peer_uids` 必须同时包含 0（root 执行 service/deploy/rollback 时探测 Worker readiness）与 `<运行账号>` 的 UID（Admin 经 control socket 调用 Worker）。
- 每次 deploy 在 `<部署状态目录>/rollback/` 增加一份四个程序的副本（rollback 增加三个角色程序的副本），每次 deploy 在 `<停机备份目录>` 增加一份停机备份，CLI 都不自动清理；部署前顺带看一眼两处占用，需要腾空间时按 `apps/auction-cli/README.md#回滚记录与停机备份的容量` 的步骤人工清理（保留最新成功回执引用的回滚记录与最近几条，整目录删除旧记录、成对删除旧备份），清理属于生产写入，先征得用户同意。
- 站点：`<Public 站点>`、`<Admin 站点>`。当前部署记录在 `<部署说明>` 的“当前部署”段；部署历史以 `<部署状态目录>/receipts/` 的回执为准。
- 版本探测用 `/proc/<MainPID>/exe` 的 SHA-256。**绝不要**运行 `auction-worker --version/--help`，那会启动一个真实 Worker。
- 不 `cat` 或复制 `secrets.toml`、`owner/` 下的文件；只读查询数据库时包在 `begin read only; … rollback;` 里。

# 可复用工具

每次新建 `<演练根>/deploy-drill-<UTC 时间戳>/`，从最近一轮演练目录复制以下脚本，用 `DRILL_ROOT=<新目录>` 运行（脚本里的默认路径指向旧目录，不能依赖默认值）：

| 脚本 | 作用 |
| --- | --- |
| `derive-config.py` | 把 `<云端配置副本>` 改成隔离演练配置：只改端点、监听、control socket 与外部子系统开关，口令与密钥不回显；支持 `DRILL_ROOT`、`DRILL_PG_PORT` |
| `run-drill.sh` | 在 netns 内完整重放部署停机序列（见下文），root 执行、角色以 `<运行账号>` 身份运行 |
| `build-drill-receipt.py <reviewer> <package-dir> <package-tarball> <selected-window-minutes>` | 只读证据生成 `evidence/migration-drill-receipt.json` |
| `after-verify.sh <evidence-dir> <deploy-started-utc>` | 在生产只读核对进程身份、readiness、schema、配置摘要、表行数、投递状态与日志 |
| `verify-live-assets.py <embedded-artifact-dir> <output-json>` | 抓取两个站点的入口与静态资源，与 payload 清单逐项比对 |
| `readme-section.template.md` | 生产 README“当前部署”段的模板 |

早期演练目录里含手写 `DROP DATABASE` 的旧 `run-drill.sh` 已作废（见该目录的 `REVISION-NOTE.md`），不得复用。

# 流程

## 1. 取得并校验制品

1. `gh release download v<版本> -D <本轮目录>/candidate-release -p 'auction-rust-*.tar.gz' -p SHA256SUMS -p DEPENDENCIES.md`。
2. 在仓库根执行 `bash ci/release/verify-deployment-package.sh <tar.gz> --tag v<版本>`。它会核对外层与包内 SHA256SUMS、`RELEASE-MANIFEST.v1` 与 `DEPENDENCIES.md` 的源码身份、构建提交能否从 tag 到达，以及 GitHub 来源证明（需要已登录的 `gh`）。不加 `--offline`；校验不过就停止。
3. 解包到 `candidate/`，记录包名、SHA-256、`source_commit` 与包内 CLI 摘要。

## 2. 生产基线（只读）

记录三个角色的 `MainPID`、`/proc/<pid>/exe` 摘要与 `NRestarts`，以及 `db status` 中的当前 schema、配置文件的摘要/属主/权限、`<程序目录>` 及其父目录与四个程序的属主/权限、`<部署状态目录>` 及其父目录的属主/权限，和表行数。用候选包的 CLI 判断本次是否含迁移（当前 `latest_version` 与候选的 `expected_version` 是否不同）。

旧程序（演练中的“旧角色”）就是线上正在运行的那个包：是 Release 时从 GitHub Release 下载，否则取上一轮演练目录里归档的 `candidate/`。两种情况都要与线上 `/proc/<pid>/exe` 摘要逐个核对。

## 3. 迁移演练（含迁移时必做，deploy 会强制核对回执）

按 `ops/deploy/MIGRATION-DRILL.md` 执行，本机要点：

- 源备份：在生产用当前 CLI `backup create --database <库名> --output /root/<证据目录>/before-drill.dump` 在线生成，连同 `.manifest` 复制到演练目录的 `source/`，并核对 SHA-256。
- 配置：把 `<云端配置副本>` 复制到演练目录，用 `derive-config.py` 改成隔离配置；维护凭据 TOML 的父目录为 700、文件为 600。
- 隔离：`sudo ip netns add <ns>`，在 `ip netns exec` 中运行 `run-drill.sh`，外部网络不可达（脚本会先证明这一点）。
- 每次运行都用**全新 PG18 集群**（`initdb -A trust`）和全新的证据、日志目录；脚本遇到非空证据目录、已存在的集群目录、超过 100 字节的 socket 路径或对 group/other 可写的 control 目录时失败关闭。
- **不得出现任何手写 `DROP DATABASE`**，隔离库也不例外。空库用正式 `auction-cli db create --database <名> --maintenance-secrets <隔离维护 TOML> --owner-secrets <owner TOML> --no-systemd` 建立。确实要重置同名库时，只经 `auction-cli db rebuild --mode empty-for-restore --backup-manifest <manifest>`：先只读预览，再加 `--execute --confirm 'REBUILD <库名>' --receipt <新文件>`。更简单的做法是换一个新的集群目录。
- 全新集群先引导两个一次性角色：演练维护角色为 LOGIN SUPERUSER，`auction_runtime` **只给 LOGIN**（带 superuser 会被授权函数拒绝）。
- 脚本序列：`db create` → 候选 `config check --strict` → `backup verify` → `db preflight` → `backup restore --dry-run` → `backup restore --receipt` → `db grants` → 旧角色逐个就绪 → 按 Public→Admin→Worker 停机并测排空 → 旧 CLI 建停机备份，新、旧 CLI 各校验一次 → 逐个原子替换程序（四个程序整体不是原子的）→ 候选 CLI `db migrate` 并计时 → 按 Worker→Admin→Public 逐个启动，readiness 200 → `db inspect-consistency`。
- 角色与 PG 都要在 netns 内启动；`pg_ctl` 用 `runuser -u <运行账号>` 执行。所有 `--output`/`--receipt` 都是 create_new，重跑必须换新目录。计时不依赖 GNU `/usr/bin/time`，用 `date +%s.%N` 加 awk。
- 回执用 `build-drill-receipt.py` 生成，格式按 `ops/deploy/migration-drill-receipt.template.json`。校验器要求：
  - `backup.verify_result="verified"`，`restore.result="passed"`；
  - `migration.result="passed"`，退出码 0，applied 等于版本差；
  - 三项 readiness 均为 `http_status=200`，且 `exit_code`、`pid`、`checked_at` 都有证据；
  - 一致性巡检（W11.4）`passed`；
  - `downtime` 时间线单调，`minimum_window_minutes ≥ ceil(3×max(实测, 估计)/60)`，`selected_window_minutes ≥ minimum`；
  - `anomalies=[]`，`conclusion.result="passed"`；
  - `operator ≠ reviewer`，可用独立的只读审核子代理担任 reviewer；
  - `evidence_files` 覆盖所有被引用的证据并附 SHA-256。
- 回执 7 天内有效（`--drill-max-age-days` 默认 7），且必须与本次制品的提交、包名和 CLI 摘要一致。

## 4. 传输

用 scp 把 Release 的 tar.gz、`SHA256SUMS` 与演练回执传到 `/root/<证据目录>/`（命名如 `auction-v<版本>-<短提交>-<时间戳>/`，下设 `package/`、`drill/`、`baseline/`），核对 SHA-256。在生产以 root 解包到 `<发布包暂存目录>/<package>/`，在包目录里跑 `sha256sum -c SHA256SUMS`。包目录保持 root 所有、禁止他人写入，**不要** `chown` 给 `<运行账号>`：deploy 从这里复制程序，执行 deploy 的也是这里的 CLI。

## 5. dry-run

必须用制品内的 CLI 执行，否则会被拒绝：

```bash
"$PKGDIR/bin/auction-cli" --config-dir <配置目录> deploy "$PKGDIR" \
  --owner-secrets <配置目录>/owner/postgres.toml \
  --state-dir <部署状态目录> \
  --worker-unit <Worker unit> --admin-unit <Admin unit> --public-unit <Public unit> \
  --backup-dir <停机备份目录> \
  --migration-drill-receipt <receipt.json> --protection-window-minutes 60 \
  --actor <name> --startup-timeout-seconds 60 --dry-run
```

逐项核对：`outcome=planned`；`shutdown.outcome=allowed`（final 场次或窗口内有在拍 lot 会阻挡，只有用户明确同意才加 `--force --reason`）；`in_flight_sends` 为空；`versions` 与 `migration_drill` 均被识别，`versions.installed_roles` 是线上程序的身份（为 null 说明找不到摘要一致的成功回执，回退时会按观测版本假设）。安装目录信任或 `--backup-dir` 属主不符时 dry-run 就会失败，按上文处理后重跑。

## 6. 正式部署

同一命令去掉 `--dry-run`。deploy 会按 Public→Admin→Worker 停机、创建停机备份并用新旧 CLI 双校验、保留回滚记录、逐个原子替换程序（root:root 0755；四个程序整体不是原子的，中途失败时把已替换的程序从回滚记录换回）、迁移，再按 Worker→Admin→Public 启动并等待 readiness，最后写入 `<部署状态目录>/receipts/<uuid>.json`。

失败处理按 `ops/README.md#升级`：除安装中途失败时换回已替换的程序外，命令不会自动回滚或重启，先用 `service status` 和 journal 确认实际状态，再决定是 `service start --owner-secrets …`（它照常执行内嵌迁移，schema 已匹配时为空操作）、`rollback --receipt-id`，还是恢复停机备份。停机后、安装前失败（停机备份、双 CLI 校验、保留旧程序），以及安装中途失败且错误含“已复核为部署前的旧程序”时，只能用 `<程序目录>/auction-cli` 执行 `service start` 恢复旧版（沿用本次的 `--config-dir`、`--owner-secrets`、`--state-dir` 与 unit 参数），不要用 `$PKGDIR/bin/auction-cli`：它会先把 schema 迁到新版本，而安装目录仍是旧程序。错误含“自动换回未完成”时按 `ops/deploy/RECOVERY.md#安装中途失败的手工换回` 处理。失败回执的 `deploy` 段只是尝试安装的制品，实际程序以回执 `artifacts` 的摘要为准；安装中途失败并已换回时本次回滚记录保留的就是当前程序，`rollback` 会因目标与当前程序相同而拒绝。

`rollback` 只换回三个角色、保留当前 CLI，必须用 `<程序目录>/auction-cli rollback …` 执行；部署在迁移完成前失败时保留的 CLI 比数据库新，rollback 会在停机前拒绝，按错误提示处理。跨迁移回退前须由用户确认把三角色共用 common 的 `[readiness] migration_compatibility_mode` 显式设为 `compatible`（rollback 不改配置，未设置时在停机前拒绝），回到同版程序后再恢复 `shadow`。

## 7. 部署后核对（任一项不符即按失败处理）

- 三个角色的 `/proc/<pid>/exe` 与制品摘要一致，`NRestarts=0`，`service … status` 返回 0 且三者 ready；`<程序目录>` 中四个程序为 root:root 0755。
- `service receipt show <id>`：`outcome=succeeded`，`deploy.roles`/`deploy.cli` 指向本次 Release、`deploy.database_version` 是部署前的版本，程序摘要、迁移版本、停机备份与演练回执都符合预期。
- `db status` 为 schema_current，没有待执行迁移。
- 两个站点外部返回 200；`verify-live-assets.py` 显示内容寻址资源与 payload 逐项一致（入口 `index.html` 由服务端渲染，不参与字节比对）。
- 配置文件的内容、属主与权限不变；表行数只允许运行态差异（`_sqlx_migrations`、`schema_migration_compatibility`、迁移与巡检审计、心跳）；部署后 journal 没有 ERROR/WARN。

## 8. 记录

- 用 `readme-section.template.md` 更新 `<部署说明>` 的“当前部署”段：制品与 Release tag、迁移、演练回执、部署回执 ID、恢复点及其 SHA-256、核对结果和回退可行性。
- 制品、演练、dry-run、部署与核对证据统一放在生产 `/root/<证据目录>/`，本机演练目录留一份副本。

# 注意

- 迁移只向前。区间内含 `breaking` 迁移时 `rollback` 会被拒绝，只能按 `ops/deploy/RECOVERY.md` 用停机备份恢复。
- 带有特殊升级说明的迁移（例如 000108 要求删除 `worker.toml` 里的 `[worker.mooglemail.mail_templates]`），先在 `ops/README.md#升级` 找到对应说明，再做 `config check --strict`。
- 部署不是业务验收：浏览器交互、外部邮件/论坛调用、数据库恢复都需要另行授权。
