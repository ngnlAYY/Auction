# auction-cli

[docs/README.md](../../docs/README.md) · [ops/README.md](../../ops/README.md) · [ops/config/README.md](../../ops/config/README.md) · [ops/deploy/RECOVERY.md](../../ops/deploy/RECOVERY.md)

Auction 的运维命令行：配置与部署预检、分体 systemd 部署的顺序启停、数据库迁移/建库/备份恢复/重建，以及凭据、规则、论坛、装备 PM、邮件证据、Bot 资金和结算异常等显式运维入口。

CLI 写入审计的 `source=cli`，并按主机追加 `entrypoint=cli`、`os_uid` 与可选 `sudo_user`；`--actor` 仍是必填的人工说明，不替代主机身份。Admin 写入保持 `source=admin`，不附加 CLI 身份。

它只装配既有的配置 loader、应用端口与存储实现，不绕过领域规则、幂等账本或追加式审计；普通服务启动不会隐式迁移、备份或删库。显式维护使用本工具；合并部署另允许在首次设置网页确认后初始化专用空库。参数的权威说明是 `auction-cli <命令> --help`。

## 构建

| feature | 作用 |
| --- | --- |
| （默认无） | 可用 `config show/check` 与 `service status`；`service stop` 无法读取时段证据，默认拒绝，只能以 `--force --reason` 声明例外；数据库、备份、业务命令和 `service start/restart` 明确报错 |
| `postgres` | 数据库、备份恢复与全部业务运维命令 |

```bash
cargo build --locked --release -p auction-cli --features postgres
```

发布包中的 `bin/auction-cli` 已按上述 feature 构建（见[ci/README.md](../../ci/README.md)）。仅支持 Linux x86_64。

## 通用约定

- **配置目录**：全局参数 `--config-dir <DIR>`（默认 `/etc/auction`）指向含 `public.toml`、`admin.toml`、`worker.toml` 的目录，分体部署与合并部署（`auction-server` 的 `--config-dir`）使用同一布局。相对路径按当前目录解析。
- **数据库来源**：数据库、凭据、邮件、资金与结算命令统一读取 `worker.toml` 及其敏感文件中的 `[database]`；三角色指向同一数据库由 `config check` 保证。连接串、口令、Cookie 与数据密钥不会出现在参数、输出或日志中。
- **输出**：成功的结构化结果以单个 JSON 文档写入 stdout；进度与诊断写入 stderr，进度前缀 `[auction-cli]`，顶层错误前缀 `auction-cli:`。失败可能没有 JSON，或已先输出状态 JSON；脚本应结合退出码判断结果，不要解析错误文本。
- **文件权限**：进程以 `umask 077` 运行；证据、回执、备份与 manifest 以 `create_new` + `0600` 写入，父目录须为非符号链接、禁止 group/other 写入，已存在的目标一律拒绝覆盖，并在报告成功前同步文件与父目录。

### 退出码

| 退出码 | 含义 |
| --- | --- |
| `0` | 成功（`--help`、`--version` 同样为 0） |
| `2` | 参数错误（包括窗口越界、force/reason 缺少配对、原因空白或格式非法），或 `config` 命令失败 |
| `3` | `db`、`backup` 命令失败；定时备份的配置、水位、锁、超时、创建、校验、异机副本或清理失败也使用此码，打卡失败不改变退出码 |
| `4` | `service` 命令失败、时段门禁拒绝/查询失败未强制，或部署未完全就绪 |
| `5` | 业务运维命令失败、被拒绝，或已提交但需要人工处理（如资金账户冻结） |

## 命令一览

| 命令组 | 子命令 | 是否写入 |
| --- | --- | --- |
| `config` | `show <role>`、`check [--strict]` | 只读文件 |
| `switch` | `list`、`set`、`clear` | set/clear 追加运行时开关事实与审计，三角色约 5 秒内生效 |
| `service` | `status`、`start`、`stop`、`restart`、`receipt list/show` | 启停 systemd unit；start/restart 执行迁移并追加部署回执；receipt 只读状态目录 |
| `deploy` | `<制品目录> [--dry-run]` | 停机、备份、迁移并原子安装 CI 制品；`--dry-run` 只核对并输出计划，见[制品部署与回退](#制品部署与回退) |
| `rollback` | `--receipt-id <deploy_receipt_id> [--dry-run]` | 换回 `deploy` 保留的旧程序、不执行迁移；中间迁移须全部登记为 `compatible`，见[回退](#回退) |
| `db` | `status`、`migrate`、`grants`、`create`、`preflight`、`inventory`、`retention-dry-run`、`retention-purge`、`inspect-consistency`、`asset-reports`、`report-solvency`、`rebuild` | migrate/grants/create/rebuild/retention-purge 写入；inspect-consistency 只追加巡检审计；retention-dry-run、asset-reports、report-solvency 只读 |
| `backup` | `create`、`verify`、`restore`、`scheduled`、`scope-extract` | restore 写数据库；scheduled 写本地备份/回执并按资格清理，可选 age 加密/R2 副本与外部打卡；scope-extract 只写新的解出目录 |
| `wal` | `archive`、`base-backup`、`upload` | archive/base-backup 写本地暂存（`archive_command` 与每周 timer 入口）；upload 读暂存、写 R2 `wal/` 与 `base/` 前缀，按规则清理本地副本 |
| `alert` | `report` | 只读主机证据；写入 `backup`/`wal` 两个运维告警来源 |
| `credential` | `list`、`import` | import 写入 |
| `keys` | `status`、`reencrypt` | reencrypt 默认演练；`--apply` 写入信封并追加审计 |
| `rules` | `import` | 写入 |
| `forum` | `recovery`、`authorize-retry` | authorize-retry 写入 |
| `pm` | `list`、`reconcile` | reconcile 写入 |
| `mail` | `export-evidence`、`inspect-sent` | 导出只读本地归档；Sent 核查还读取数据库与远端邮件，结果写入本地文件 |
| `funds` | `preview`、`open-record-only`、`reconcile` | 后两者写入 |
| `settlement` | `equipment-loss <LOT_UUID>`、`confirm-sent <DELIVERY_UUID> --evidence <JSON>` | 默认只读；`--execute` 记录相应人工处置或 Sent 证明 |
| `recovery` | `external-reconcile` | 默认只读扫描；`--apply` 把精确 Sent 匹配隔离为人工对账，见[恢复后外部核对](#恢复后外部核对) |

### 数据密钥环

```bash
auction-cli keys status --strict
auction-cli keys reencrypt --domain credential                       # 演练，不写库
auction-cli keys reencrypt --domain credential --apply \
    --actor ops@example --receipt /var/lib/auction/keys/credential-20261105.json
```

`status` 报告各域当前/旧 key ID、数据库实际引用的 key ID 与 12 个月保留期；`--strict` 在存在未配置 key 引用或过期未重加密的旧 key 时失败。`reencrypt` 只处理 `equipment`、`user_login`、`credential` 三个数据库域（省略 `--domain` 时依次处理）：先解密旧信封、用当前 key 重新封装并重新解密核对，`--apply` 时以旧 key ID 作 CAS 条件写回并追加 `system.data_key_reencrypt` 审计。重复运行是幂等的；`mail_diagnostics` 归档是一次写入的加密文件，不重写。流程与保留期约束见[ops/keys/README.md](../../ops/keys/README.md)。

## 配置检查

```bash
auction-cli --config-dir /etc/auction config show worker
auction-cli --config-dir /etc/auction config check --strict
```

`config show` 加载单个角色并输出有效配置的安全投影：文件路径、监听、数据库后端、可观测性开关、HTTP 安全、会话、拍卖模板摘要、Worker 子系统开关与密钥“是否配置”，不输出任何秘密值。

`config check` 联合加载三角色，核对：三个角色文件、`common_file`、`secrets_file`、`runtime_config_file` 均为非符号链接普通文件，且三角色引用同一 `common_file` 与同一（或全部省略的）`runtime_config_file`；三角色数据库为同一 PostgreSQL 目标且 `equipment_data_key` 标识与材料一致；Admin/Worker 的拍卖模板与排期、control socket、四个域的密钥与旧密钥数组一致，Admin 若配置 `worker_config_file` 必须指向本目录的 `worker.toml`；装备规则与 `bot_players` 名单一致；Worker 备份策略范围与可选打卡地址合法，报告只给“是否配置”；Public/Admin 监听合法且不与 Worker control socket 冲突；配置目录与普通文件禁止 group/other 写入、敏感文件仅 owner 可访问，runtime 文件必须是 `0640` 且父目录禁止他人写入。非严格模式把权限类问题放进 JSON `warnings`，`--strict` 直接失败；生产部署门禁必须使用 `--strict`。字段语义与各区块约束由[ops/config/README.md](../../ops/config/README.md)拥有。

## 分体部署编排

`service` 只用于分体 systemd 部署（三个角色各自一个 unit）。合并部署（`auction-server`）由 supervisor 自行编排；`service start/restart` 发现该配置目录对应的 `owner.lock` 时拒绝执行。`status/stop` 仍查询或停止所指定的分体 unit，不能用于管理 supervisor。

```bash
auction-cli --config-dir /etc/auction service status
auction-cli --config-dir /etc/auction service start --startup-timeout-seconds 30
auction-cli --config-dir /etc/auction service restart
auction-cli --config-dir /etc/auction service stop
# 按演练回执设置本次窗口；默认 60 分钟
auction-cli --config-dir /etc/auction service restart --protection-window-minutes 90
# 人工例外，原因会进入输出/journal，不能填写秘密或玩家信息
auction-cli --config-dir /etc/auction service stop --force --reason '紧急修复主机磁盘故障'
```

| 参数 | 默认值 |
| --- | --- |
| `--systemctl` | `/usr/bin/systemctl` |
| `--state-dir`（部署控制锁目录） | `/var/lib/auction/deployment` |
| `--worker-unit` / `--admin-unit` / `--public-unit` | `auction-worker.service` / `auction-admin-api.service` / `auction-public-api.service` |
| `start/restart --startup-timeout-seconds` | 每角色 readiness 的完整预算（状态查询、探测、进程身份复核及轮询等待），默认 30 秒，范围 1–300；`systemctl start` 本身另有 90 秒上限 |
| `stop/restart --protection-window-minutes` | 本次调用的截止保护窗口，默认 60 分钟，范围 1–10080（7 天）；运维须依据[ops/deploy/MIGRATION-DRILL.md#停机时长与窗口](../../ops/deploy/MIGRATION-DRILL.md#停机时长与窗口)设为至少实测停机时长的 3 倍 |
| `stop/restart --force --reason <文本>` | 默认不强制；两者必须配对，原因须为 1–1024 字节、首尾无空白且不含控制字符的单行文本 |
| `start/restart --backup-id <ID>` | 可选；本次部署核对过的恢复点标识，1–256 字节、首尾无空白且不含控制字符，仅写入回执 |
| `start/restart --actor <NAME>` | 可选；写入回执的操作者，1–128 字节；缺省依次取 `SUDO_USER`、`USER`、`LOGNAME`，都不可用时记为 `uid:<euid>` |

- **start**：取得部署锁（`<state-dir>/deployment-control.lock`，目录不存在时以 0700 创建）→ `config check --strict` → 核对三个 unit 均已装载、`Environment=AUCTION_CONFIG_FILE` 与 `--config-dir` 中的角色文件是同一文件、Admin runtime 文件与父目录归 Admin unit 的固定 `User` 所有（不接受 `DynamicUser`）→ 三角色全部运行时只按顺序复核 readiness 并返回 `already_running`；部分运行或处于转换状态则拒绝 → 检查 TCP 监听端口可绑定 → 执行迁移 → 按 Worker → Admin → Public 逐个 `systemctl start`，等待 unit 为 active 且应用 readiness 返回 HTTP 200，探测前后 `MainPID` 与 `InvocationID` 不变。任一角色失败时反序停止本次启动的全部 unit；已提交的迁移不会回退。成功、`already_running` 与失败都会追加部署回执。
- **readiness**：Public/Admin 探测 `GET /health/ready`（TCP 或 Unix socket），Worker 探测 control socket 的 `GET /v1/health/ready`。Worker control 只接受 `allowed_peer_uids/gids` 中的调用者，**以 root 运行 `service` 时须把 `0` 加入 `allowed_peer_uids`**，否则 Worker 永远无法判定就绪；因此 `service start` 也要求启用 `[worker.control]`。
- **restart**：先完成与 start 相同的预检（新配置不可用时不停止现有服务），再读取只读时段证据；允许或已声明人工例外后，按 Public → Admin → Worker 停止，随后迁移并顺序启动，并追加与 start 相同的部署回执。
- **stop**：取得部署锁，读取只读时段证据；允许或已声明人工例外后，按 Public → Admin → Worker 逐个停止，并确认每个 unit 已停止后才处理下一个。
- **status**：不取锁、不要求严格权限；输出每个 unit 的 `load_state/active_state/sub_state/main_pid` 与 readiness，任一角色未就绪即以 4 退出。配置无法加载时仍报告 systemd 状态，并在 `config_error` 中给出原因。
- systemctl 调用均有超时（查询 10 秒、启动 90 秒、停止为 unit 的有限 `TimeoutStopUSec` + 10 秒）；超时只终止 systemctl 客户端，已提交给 systemd 的 job 可能仍在执行，须重新查看状态。

[ops/deploy/systemd/auction-stack.service.example](../../ops/deploy/systemd/auction-stack.service.example) 把 `service start/stop` 包装为 oneshot unit，用于开机按顺序拉起。

### 部署回执

`service start/restart` 通过部署锁与预检后，在迁移和启停完成时把本次结果追加到 `<state-dir>/receipts/<receipt_id>.json`；`already_running`、启动失败、readiness 超时与回滚不完整也留下回执。回执取代在服务器上手工维护的部署 README，是部署历史的查询入口：

```bash
# --state-dir 是 service 的参数，须写在 receipt 子命令之前
auction-cli service --state-dir /var/lib/auction/deployment receipt list --limit 20
auction-cli service --state-dir /var/lib/auction/deployment receipt show <receipt_id>
```

查询只读状态目录：不取部署锁、不连接数据库、不要求 systemctl。目录不存在表示还没有回执；损坏、未知格式、重复 `receipt_id`、符号链接或文件名与内容不符时失败关闭，不返回部分记录。

格式 `auction-deployment-receipt.v1` 的闭集字段：

| 字段 | 内容 |
| --- | --- |
| `format` | 固定 `auction-deployment-receipt.v1` |
| `receipt_id` | UUID；文件名 `<receipt_id>.json` |
| `action` | `start` / `restart` |
| `outcome` | `succeeded` / `already_running` / `failed` |
| `started_at` / `finished_at` | UTC 时间；完成时间不得早于开始时间 |
| `actor` / `actor_uid` | 操作者名称与有效 UID；名称来自 `--actor` 或环境，缺省 `uid:<euid>` |
| `backup_id` | `--backup-id` 原值；未提供为 `null`，不触发备份或恢复 |
| `migration` | `before_version`/`after_version`/`expected_version`/`applied`；未执行迁移（含 `already_running`）为 `null` |
| `artifacts` | 实际运行角色的 `/proc/<pid>/exe` 路径与 SHA-256/字节数；进程不可读时摘要与字节数为 `null`，不伪造已验证身份 |
| `failure` | 失败摘要（≤1024 字符的单行文本）；成功时为 `null` |

回执只追加：目录以 0700 创建，内容先写入 0600 临时文件并 fsync，再以 `RENAME_NOREPLACE` 发布；同一 `receipt_id` 不覆盖，更正只能形成新回执。目录默认 `<state-dir>/receipts`；把 `common.toml [backup.scope].deployment_receipts_dir` 指向该目录即可随备份范围加密归档。回执不参与启停决策，也没有独立状态机；`service stop` 不写回执。

### 停机时段门禁

在首个 `systemctl stop` 前，从 Worker 数据库读取同一可重复读只读快照，不加行锁/advisory lock、不写入。仅以下业务条件阻挡：

1. `public.auctions.status = 'final'` 的任一场次，不根据排期推测 Final，也不因其截止已过而放行。
2. `public.lots.status = 'live'` 且 `effective_end_at <= protected_until`（数据库检查时刻加保护窗口），包括已过截止但生命周期尚未关闭的 lot，等于窗口末端也阻挡。软截止与提前结算均以持久化的 lot 有效截止为准，不使用场次名义结束时间。

慢速代理的调度来源是 `lots.slow_due_at`，它本身不触发门禁；只有其 lot 满足上述截止条件才计入。Live 的其余时段允许重启，持久化代理任务在恢复后顺延执行。查询、连接、配置加载或行解码失败（含 live lot 缺截止）默认失败关闭，退出码 4，尚未发出任何停止请求。`start` 不查此门禁。

`--force --reason` 只绕过时段阻挡或证据不可用，仍读取并记录能取得的证据，不绕过 restart 的配置/unit 预检、部署锁、停止确认、迁移或 readiness。已过截止却长期仍为 live 的 lot，须先人工排查生命周期与收尾故障，再以 `--force --reason` 声明例外。窗口用 CLI 参数而非角色 TOML：这是本次运维停机的预算，调用时可按演练结果设置，无需重启角色；CLI 只校验数值范围，不自动读取演练回执或保证三倍关系。systemd 包装调用默认用 60 分钟；需要更大窗口时由运维在包装命令中显式传参。

停止前向 stderr 输出一行 `stop/restart 停机时段门禁：<JSON>`，正常保存到 journal；内容为 `outcome=allowed/blocked/forced`、`protection_window_minutes`、`force`、`reason`、`snapshot`、`query_error`。snapshot 只含数据库 `checked_at/protected_until`、`final_auctions`（场次 ID 与有效截止）和 `ending_lots`（场次/lot ID 与有效截止），无玩家信息；证据不可用时 snapshot 为 null，query_error 为安全分类。未强制的拒绝还输出单个 stdout JSON（`action=stop/restart`、`outcome=blocked`、`shutdown_gate`），然后以 4 退出。成功 stop 的 JSON 增加 `shutdown_gate`；成功 restart 沿用既有 `action=start`、`outcome/migration/roles` 并增加 `shutdown_gate`。后续停止/迁移/启动失败可能没有 stdout 结果，停止前的 stderr 声明仍保留；这里不新增持久回执文件。

`query_error` 的配置加载失败只附固定 `kind`（`config_read/config_parse/config_invalid/config_load`），连接失败只附 `StorageError` 变体的固定分类；时段查询失败附固定 `stage`、SQLx 变体名 `sqlx_kind` 和经过校验的 `sqlstate`（恰好 5 位 ASCII 大写字母或数字，缺失为 `none`、非法为 `invalid`）。不输出配置值、连接串、SQL 或驱动原文，也不保留底层错误链。

门禁是检查时刻的快照，不阻止并发竞价或生命周期推进，也不能拦截直接 systemctl、合并部署、主机重启与系统自动更新；这些绕过 CLI 的停机边界由主机加固基线约束，见[ops/README.md#停机时段保护](../../ops/README.md#停机时段保护)。

## 制品部署与回退

`deploy` 把一份解压后的 CI 发布包换成线上运行程序，只支持分体部署；合并部署的支持另行决定。命令必须由制品内的 `auction-cli` 执行：迁移集合与期望 schema 版本来自正在运行的二进制，因此运行程序的 SHA-256 必须等于制品 `bin/auction-cli`，否则在任何停机前拒绝。

```bash
# 先演练：制品、配置、unit 绑定、时段门禁、在途发送、迁移版本与演练回执全部核对，输出计划
/opt/auction-run/auction-rust-<版本>-<提交>/bin/auction-cli --config-dir /etc/auction deploy \
  /opt/auction-run/auction-rust-<版本>-<提交> \
  --owner-secrets /etc/auction/owner/postgres.toml \
  --backup-dir /var/backups/auction --dry-run

# 正式执行（含迁移时必须提供本次制品对应的迁移演练回执）
/opt/auction-run/auction-rust-<版本>-<提交>/bin/auction-cli --config-dir /etc/auction deploy \
  /opt/auction-run/auction-rust-<版本>-<提交> \
  --owner-secrets /etc/auction/owner/postgres.toml \
  --migration-drill-receipt /srv/auction-drill/receipts/<drill>.json
```

执行顺序：

1. **制品静态校验**（离线）：`RELEASE-MANIFEST.v1`（schema、包名、来源提交/指纹、`files=` 白名单与权限）、`SHA256SUMS`（除自身外的全部成员摘要）、`docs/DEPENDENCIES.md`（来源提交与 `Source worktree: clean`）、`docs/WEB-{PUBLIC,ADMIN}-SHA256` 与 `docs/WEB-*-MANIFEST.v1`（`payload_sha256` 必须等于摘要清单文件的 SHA-256，双端不得相同），以及四个程序的 0755/非空/摘要。符号链接、特殊文件、白名单外的文件、`-dirty`/`-single` 包名一律拒绝。
2. **严格预检**：与 `service restart` 相同（配置、unit 装载与 `AUCTION_CONFIG_FILE` 绑定、Admin runtime 属主、`[database_roles] owner` 声明）；合并部署 `owner.lock` 存在时拒绝。安装目录缺省由三个运行进程的 `/proc/<pid>/exe` 推导，三者必须同目录；都已停止时必须显式传 `--install-dir`。
3. **停机时段门禁**：与 `service stop/restart` 同一只读证据与 `--force --reason` 语义，`action=deploy`。
4. **版本与迁移演练回执**：读取数据库版本。制品期望版本低于数据库时拒绝（回退请用 `rollback`）；高于数据库时必须提供 `--migration-drill-receipt`，回执须为 `auction-migration-drill.v1`，且候选提交/包名/CLI 摘要、备份 schema 与校验结果、`before → after` 迁移区间、三角色 readiness、结论与停机窗口下限与本次部署一致；执行人与审核人必须不同，`anomalies` 必须为空，`consistency_check.work_item` 为 `W11.4`（回执格式固定值）且 `result` 为 `passed` 并附 `db inspect-consistency` 证据，不再接受 `not_available`。
5. **在途发送门禁**：只读查询 `deliveries` 与 `lot_watch_pm_deliveries` 中状态为 `sending` 且租约未过期的记录，最多等待 `--send-gate-timeout-seconds`（默认 30，0 表示只查一次）；超时拒绝并列出内部标识。不能用停机打断已越过 sending 围栏的外部写入。
6. **停机与恢复点**：按 Public → Admin → Worker 停止并逐个确认；然后用 Worker 配置创建停机备份（`<backup-dir>/auction-deploy-<UTC>.dump` 与配对 manifest），并分别用**安装目录中的旧 CLI**和**制品中的新 CLI**执行离线 `backup verify`，任一失败即停止流程。
7. **保留旧程序**：把当前四个程序连同 SHA-256/字节数写入 `<state-dir>/rollback/<deploy_receipt_id>/`，版本区间记录部署前后的数据库版本；记录只追加，不覆盖。
8. **安装与迁移**：以同目录临时文件 + `rename` 原子替换 `--install-dir` 中的四个程序，属主设为 unit 的固定服务账号（`DynamicUser` 拒绝），安装后复核摘要；随后执行与 `service start` 相同的迁移与 Worker → Admin → Public 顺序启动/readiness。
9. **运行时核对**：三角色 `/proc/<pid>/exe` 的 SHA-256 必须等于制品对应程序，这也是内嵌 Web payload 已生效的证明。
10. **回执**：成功追加 `action=deploy` 的部署回执；失败同样追加 `failed` 回执（若能取得制品证据则一并记录）。安装后启动失败只停止本次启动的 unit，不会自动换回旧程序；错误文本给出回滚记录标识。含迁移的版本回退必须经过第 4 步的区间判定，见下节。

`--dry-run` 只执行第 1–5 步（含只读数据库门禁），输出 `outcome=planned` 与 `planned_steps`，不创建备份、不停机、不安装、不写回执、不调用 `systemctl start/stop`。演练回执里 `migration.applied` 必须等于 `after - before`，`downtime.minimum_window_minutes` 不得低于实测/估算停机秒数的三倍，且本次 `--protection-window-minutes` 不得低于该下限。

### 回退

```bash
auction-cli --config-dir /etc/auction rollback --receipt-id <deploy_receipt_id> --dry-run
auction-cli --config-dir /etc/auction rollback --receipt-id <deploy_receipt_id>
```

`rollback` 只换回 `deploy` 保留的旧程序，**不执行迁移**：

- 回滚记录必须存在且保留程序摘要仍与记录一致；安装目录中的程序必须仍是该记录对应部署回执里的版本，否则拒绝（先核对安装目录与 unit `ExecStart`）。
- 时段门禁与在途发送门禁与 `deploy` 相同。
- 数据库高于回滚目标版本时，`previous_version → 当前版本` 的每个迁移必须连续、成功且登记为 `compatible`；任一 `breaking` 或缺少登记即拒绝，并提示按[ops/deploy/RECOVERY.md#程序回退](../../ops/deploy/RECOVERY.md#程序回退)恢复升级前备份并核对外部历史。
- 安装前会把当前程序保留为**新的**回滚记录，因此回退本身可以再前进；回退成功后追加 `action=rollback` 回执，`deploy` 段指向被恢复版本的制品证据。
- `--dry-run` 只做记录、安装目录、兼容区间与门禁核对。

### 部署回执扩展

`action` 增加 `deploy` / `rollback`。成功或 `already_running` 的这两个动作必须有 `deploy` 段：

| 字段 | 内容 |
| --- | --- |
| `artifact_directory` / `install_directory` | 制品目录与程序安装目录的绝对路径 |
| `package` | `RELEASE-MANIFEST.v1` 的发布包名 |
| `source_commit` / `source_fingerprint` | 制品来源提交与源码指纹 |
| `cli_sha256` | 制品 `bin/auction-cli` 的 SHA-256 |
| `public_payload_sha256` / `admin_payload_sha256` | 两端内嵌 Web payload 摘要 |
| `migration_drill_id` | 本次核对过的演练回执编号；无迁移时为 `null` |
| `backup_archive` / `backup_manifest` | 停机恢复点的绝对路径；失败回执可为 `null` |
| `rollback_record_id` | 本次部署保留的旧程序记录；回退回执为“再次前进”的记录 |

旧回执没有 `deploy` 字段，仍按原格式解析；`service start/restart` 回执不得出现该字段。

## 停机门禁

`db migrate`、`db grants`、`db create`、`db rebuild` 与 `backup restore` 在写数据库前取得停机证明：

1. 若 `<config-dir>` 的同级运行目录 `.<目录名>.server/owner.lock` 存在（合并部署），以非阻塞方式取得其独占锁；`auction-server` 或其子进程仍在运行时失败。合并部署必须用与 `auction-server` 相同的服务账号执行，锁文件须为该账号所有的单链接 owner-only 文件。
2. 否则先取得部署控制锁（`--state-dir` 不存在时以 0700 创建），再查询三个 systemd unit，全部为 `inactive`、`failed` 或 `not-found` 才继续；目录首次创建期间也与 `service` 命令互斥，目录或锁无法安全创建时拒绝维护。
3. `--no-systemd` 显式声明本机不用 systemd 管理三角色（容器或开发环境），跳过第 2 步；合并部署锁检查仍然生效。

这些参数与 `service` 相同（`--systemctl`、`--state-dir`、`--*-unit`）。门禁只能证明进程已停止，不能阻止外部客户端连接数据库；恢复与重建另有“零其他会话”检查。

## 数据库

### 状态与迁移

```bash
auction-cli db status
auction-cli db migrate --owner-secrets /etc/auction/owner/postgres.toml
```

`db status` 只读输出目标库身份、服务器版本、是否只读/recovery、其他会话数、库大小、schema 身份与迁移历史；schema 不是当前二进制的期望版本时以 3 退出。

`db migrate` 在门禁后以**迁移 owner**（`--owner-secrets`）执行二进制内嵌的全部追加式迁移（SQLx 迁移锁内），输出迁移前后版本。空库会建立完整 schema；存在未知用户对象的非空库被拒绝；历史 checksum 不匹配、失败记录或缺失版本同样失败。迁移只向前，不提供回滚。服务运行账号（`auction_runtime`）没有 DDL 权限，不能用于本命令；`--owner-secrets` 必须指向 Worker 配置中的同一数据库，账号必须与 `[database_roles] owner` 一致且不同于服务账号，否则命令在连接前失败。

迁移 `000081` 把运行账号的最小权限固化为 `public.apply_auction_runtime_privileges()`（授权定义只此一份），`db grants` 用同一函数在恢复后或授权变更时重放：

075 起每个新迁移同事务追加兼容登记；readiness 的 compatible 模式不放宽本 CLI 的迁移、preflight、备份或 inventory 版本边界。程序回退时保留能识别当前数据库的新版 CLI，只换回已包含迁移兼容登记的旧角色程序，事先显式设置 `migration_compatibility_mode = "compatible"`；不承诺旧 CLI 的 `service start/restart` 能接受未来数据库，也没有自动 rollback 命令。完整步骤见[ops/deploy/RECOVERY.md#程序回退](../../ops/deploy/RECOVERY.md#程序回退)。

### 重放运行账号权限

```bash
auction-cli db grants --owner-secrets /etc/auction/owner/postgres.toml
```

以迁移 owner 重放 `public.apply_auction_runtime_privileges()`：重建 `public` schema 的 `USAGE`、可变表读写、只追加表仅 `INSERT`、序列 `USAGE`/`SELECT`，并恢复默认权限。备份恢复使用 `--no-owner --no-privileges`，ACL 与默认权限都不在 dump 中，因此 `backup restore` 会在核对 schema 后自动调用该函数；手工换库、手工授权变更或需要重新收口权限时使用本命令。数据库尚未执行迁移 081 时命令明确失败并提示先 `db migrate`。

### 建库

```bash
auction-cli db create --database auction \
  --maintenance-secrets /etc/auction/maintenance/postgres.toml \
  --owner-secrets /etc/auction/owner/postgres.toml
```

首次部署时用独立维护凭据创建应用数据库：`--database` 须与 Worker 配置的库名一致，库 owner 是 `--owner-secrets` 中的迁移 owner（不是服务账号）。维护凭据文件（模板见 `ops/config/examples/secrets/postgres-rebuild.toml`）使用 `[database]` 五字段，也兼容与之互斥的 `database_url`；维护库名固定为 `postgres`，主机与端口须与应用库相同。目标提取复用实际 SQLx 连接选项：`hostaddr` 按查询顺序覆盖 TCP host，已指定的 Unix socket 优先于后续 TCP host；socket 路径区分大小写，IP 地址按规范化地址比较。文件必须是规范化绝对路径、非符号链接、单硬链接、归当前账号所有且仅 owner 可读，父目录禁止 group/other 写入。

命令取得集群级 advisory lock，要求连接可写主节点；目标不存在时校验维护角色具备 `CREATEDB`（或 superuser）、owner 角色存在且可被指派为 owner，然后 `CREATE DATABASE` 并用 owner 账号回连验证。目标已存在时只校验 owner 与 owner 连接，返回 `already_exists`。本命令不迁移、不修改已有库，随后需执行 `db migrate --owner-secrets`。

### 恢复预检与保留盘点

```bash
auction-cli db preflight --database auction --output /var/backups/auction/preflight-2026-10-30.json
auction-cli db inventory --database auction --output /var/backups/auction/inventory-2026-10-30.json
auction-cli db retention-dry-run --database auction --output /var/backups/auction/retention-dry-run-2026-11-05.json
```

两者都在只读、可重复读事务中执行，`--database` 必须与连接实际的 `current_database()` 一致，完整证据写入 `--output`，stdout 只给摘要。

- `preflight`：报告是否可写主节点、其他会话数、用户对象数、schema 身份与迁移历史；`restore_preflight_passed=true` 只表示“空库、无其他连接、可写主节点”，不代表任何备份可用。
- `inventory`：读取迁移/schema 身份、public 关系规模与外键、realtime 水位，并对照编译期内嵌的[ops/deploy/retention-policy.v2.json](../../ops/deploy/retention-policy.v2.json)输出各数据族统计与政策 SHA-256。`eligibility_assessed`/`eligibility_authorized` 恒为 false，结果不授权任何归档或删除。政策的 `database_schema_version` 必须等于当前最高迁移版本，新增迁移时须同步审阅该政策。
- `retention-dry-run`：按政策 v2 的期限只读统计运行态数据族的候选行（进程心跳、realtime 事件/已解决失败、会话）与加密邮件诊断文件的候选/保全数量，回执含 `run_id`、政策身份、阈值边界、`blocked_rows` 与被排除的有效/永久会话数。全程 `REPEATABLE READ` 只读、30 秒语句上限，只执行 SELECT；迁移历史不是当前完整前缀时拒绝运行。`mutation_executed`/`deletion_authorized` 恒为 false，候选数不授权删除。目录不存在时文件族按空快照记录 `directory_present=false`；读取权限或格式异常失败关闭。
- `retention-purge`：唯一执行保留删除的入口。它读取上面写出的 owner-only 回执，要求回执不超过 15 分钟、数据库名/run id/政策版本与 SHA-256 全部一致，且数据库迁移前缀等于当前最高版本；不满足时在删除任何行或文件前拒绝。省略 `--execute` 时只打印每个数据族的候选计数与 `approval_token`，不改动任何数据。实际执行必须逐族 `--approve <family>=<token>`（令牌绑定数据库、演练 run id、政策摘要、族与表级计数）、逐字 `--confirm 'PURGE <database>'` 并给出 `--receipt`。

```bash
auction-cli db retention-purge --database auction \
  --dry-run-receipt /var/backups/auction/retention-dry-run-2026-11-05.json
auction-cli db retention-purge --database auction \
  --dry-run-receipt /var/backups/auction/retention-dry-run-2026-11-05.json \
  --approve service_heartbeat=<token> --approve realtime_events=<token> \
  --execute --confirm 'PURGE auction' \
  --receipt /var/backups/auction/retention-purge-2026-11-05.json --batch-size 500
```

删除只覆盖政策 v2 的代码内闭集表与加密诊断文件；审计、资金、结算、拍卖历史、身份与配置表没有删除路径。有效/永久会话、未解决失败、realtime 写入位置与写入未知（含超过 90 天保全窗）诊断文件永不删除；被失败记录或重放关系引用的事件先由已解决失败的清理解除引用。每批一个独立事务（默认 500 行，1–5000），使用 30 秒语句与 2 秒锁上限；批准计数是硬上限，漂移候选留待重新演练并以 `partial_limit_reached` 报告。批次失败保留已提交批次。回执（`auction-retention-purge-receipt.v1`）记录每表批准/删除行数、`pg_column_size` 行载荷字节、批次数、剩余/阻断候选与文件实际删除数与字节，并追加审计 `auction.retention.purge`；执行失败时仍写回执并以 `db` 命令失败码 3 结束。

### 破坏性重建

```bash
# 只读演练（默认）：持锁预检并输出预检证据
auction-cli db rebuild --database auction \
  --maintenance-secrets /etc/auction/maintenance/postgres.toml \
  --owner-secrets /etc/auction/owner/postgres.toml \
  --mode empty-for-restore --backup-manifest /var/backups/auction/auction-20261030.dump.manifest

# 实际执行
auction-cli db rebuild --database auction \
  --maintenance-secrets /etc/auction/maintenance/postgres.toml \
  --owner-secrets /etc/auction/owner/postgres.toml \
  --mode empty-for-restore --backup-manifest /var/backups/auction/auction-20261030.dump.manifest \
  --execute --confirm 'REBUILD auction' --receipt /var/backups/auction/rebuild-20261030.json
```

- 必须提供已通过校验的 `--backup-manifest`，或用 `--ephemeral` 明确声明目标是可丢弃的一次性库（此时确认短语为 `EPHEMERAL <库名>`）。`--mode empty-for-restore` 强制要求备份，并在 DROP 前复用 `backup restore` 的版本门禁，拒绝高于当前二进制支持版本的备份；通过后只建立空库等待恢复。`--mode fresh-schema`（默认）建库后立即执行全部迁移。
- 维护连接取得与建库相同的 advisory lock，并要求：维护库为 `postgres`、PostgreSQL 主版本 18、可写主节点、目标允许连接且非模板、无自定义 ACL 或库级设置、UTF8 编码、维护角色为 superuser 或“目标 owner + CREATEDB”、目标零活动会话、无预备事务、无活动逻辑复制槽与订阅。
- 执行时在 DROP 前复核恢复点与目标身份，以原 owner、编码、locale provider、tablespace 与连接上限 `DROP DATABASE … WITH (FORCE)` 后 `CREATE DATABASE … TEMPLATE template0`。
- 一旦进入 DROP，无论成败都写入 `--receipt`，记录 `database_dropped`/`database_recreated`（`null` 表示远端结果未知）。未知结果不能推断为未执行，禁止盲目重试 DROP；保持全部角色停止并人工核对。

## 备份与恢复

```bash
auction-cli backup create --database auction --output /var/backups/auction/auction-20261030.dump
auction-cli backup verify --manifest /var/backups/auction/auction-20261030.dump.manifest --database auction
auction-cli backup restore --manifest /var/backups/auction/auction-20261030.dump.manifest --database auction \
  --owner-secrets /etc/auction/owner/postgres.toml --dry-run
auction-cli backup restore --manifest /var/backups/auction/auction-20261030.dump.manifest --database auction \
  --owner-secrets /etc/auction/owner/postgres.toml --receipt /var/backups/auction/restore-20261030.json
auction-cli backup scope-extract --manifest /var/backups/auction/auction-20261030.dump.manifest --database auction \
  --identity /offline/age-identity.txt --output-dir /srv/restore/scope-20261030
```

- 依赖部署环境 `PATH` 中的 `pg_dump`/`pg_restore`，主版本不得低于服务器（PostgreSQL 18）。连接参数投影复用实际 SQLx 连接选项的有效 host/socket、port、database 与 user；仅传最终 `PGHOST`，不独立保留 `PGHOSTADDR`，避免预检与外部工具因查询覆盖规则不同而连接不同实例。继承的连接相关 `PG*` 环境全部清除，显式口令只写入 0600 临时 passfile 并在命令结束后删除，从不进入进程参数；TLS 与显式口令仍从敏感配置提取，不承诺与 SQLx 的全部环境默认值或隐式凭据来源一致。连接串只能含 `host`、`hostaddr`、`port`、`dbname`、`user`、`password`、`sslmode`、`sslrootcert`、`sslcert`、`sslkey`、`application_name`（另兼容 SQLx 的 TLS 别名与 `statement-cache-capacity`），`options` 与未知参数被拒绝。
- `create`：`--output` 必须以 `.dump` 结尾，文件名不得含反斜杠、控制字符或首尾空白，archive 与同目录 `<archive>.manifest` 均不得已存在。备份源须有 Auction schema 身份与完整迁移历史；`pg_dump --format=custom --no-owner --no-privileges` 写入临时文件，经 `pg_restore --list` 解析并同步 archive 文件后以硬链接无覆盖发布，随后同步父目录并写入 manifest；备份前后 schema 版本不一致则丢弃 archive。
- manifest 格式为 `auction-postgres-backup.v1`，记录 archive 文件名、archive SHA-256、库名 SHA-256、schema 版本与创建时间；此前运维工具生成的同格式备份可直接校验与恢复。
- `create` 在 `[backup.scope]` 启用时还会收集运行配置、邮件诊断证据与部署回执，打包为 `<archive>.scope.bundle.age` 并写下 `<archive>.scope.json`；这一步失败时 archive/manifest 保留但不带范围 sidecar。
- `verify`：不连接数据库，校验 manifest 字段闭集、库名摘要、archive SHA-256 与 archive 目录；存在备份范围 sidecar 时同时校验配对名称、密文字节数与 SHA-256，并在 `scope` 字段输出不含路径的投影。
- `scope-extract`：`[backup.scope]` 启用时生成的数据库之外材料（运行配置、加密邮件诊断证据、部署回执）。先用 `--identity` 指定的离线 age 私钥解密 `<archive>.scope.bundle.age` 到输出目录内的私有临时文件，核对 sidecar 的整体摘要、文件数与逐条摘要，确认无误后才把每个文件以 0600 写入全新目录；目标目录必须不存在或为空，拒绝覆盖与符号链接，临时明文在校验后删除；中途失败可能留下已写文件，不要把它们混入生产目录。与 `verify` 一样需要 PATH 中有 `pg_restore`。回执格式 `auction-backup-scope-extract.v1` 只含摘要、来源类型、文件数与目录。
- `restore`：通过停机门禁；备份 schema 版本不得高于当前二进制；目标必须是同名空库（零其他会话、可写主节点、无用户对象、无 schema 身份与迁移历史，可先用 `db rebuild --mode empty-for-restore` 准备）。恢复前再次校验恢复点，以 `pg_restore --single-transaction --exit-on-error --no-owner --no-privileges` 执行，完成后核对 schema 身份与迁移版本等于备份版本，再写入 `auction-postgres-restore.v1` 回执。恢复较旧备份后按提示执行 `db migrate --owner-secrets`。
- 恢复以 `--no-owner --no-privileges` 执行，对象 owner 与 ACL 都不在 dump 中：恢复后对象归迁移 owner，回执的 `ownership_restored=false`。备份版本已含`000081` 时命令会在核对 schema 后重放运行账号授权并记录 `runtime_privileges_applied=true`；缺失授权函数或以旧版本恢复时保持停机并提示先 `db migrate`。

完整的灾备流程见[ops/deploy/RECOVERY.md](../../ops/deploy/RECOVERY.md)。
含迁移版本上线前另须完成[ops/deploy/MIGRATION-DRILL.md](../../ops/deploy/MIGRATION-DRILL.md)，保存[ops/deploy/migration-drill-receipt.template.json](../../ops/deploy/migration-drill-receipt.template.json)；M1 不自动核对回执。

### 定时备份

```bash
auction-cli --config-dir /etc/auction backup scheduled --database auction \
  --output-dir /var/backups/auction/scheduled --state-dir /var/lib/auction/backup
```

三个参数均必填。输出与状态目录必须为不含 `..`、无符号链接的绝对路径，彼此不重叠，归执行账号所有且为 0700；缺失时创建。用固定的执行账号、输出目录与状态目录，避免把一套备份的清理证据拆散。运行账号须能读取 Worker 角色文件及其引用的 common、敏感与可选 runtime 文件，并在 PATH 中提供 PG18 客户端工具；启用异机副本时还须提供主机 `age`，服务器只放收件人公钥。

命令先取得状态目录独占锁，再收口残留运行记录；取锁失败只写 stderr 并退出 3，不创建运行记录或回执。持锁后初始化本次运行记录，调用与手动 create 相同的 Worker loader，再执行 `validate_backup()`；不联合加载 Public/Admin。随后在总预算内取得输出目录锁，检查磁盘 →（启用异机时 age 版本预检）→ create →（启用备份范围时打包并加密范围证据、写下 sidecar）→ 再检查磁盘 → verify（同时校验范围 sidecar）→（本次加密及日/月副本、逐项重试旧项）→ 本地保留清理 → 发布终态回执 → 可选打卡。create 消费同一次加载得到的 Worker 快照。备份只读取数据库，不停服、不迁移、不执行恢复。本地策略和可选 `[backup.offsite]` 见[ops/config/README.md#定时备份策略](../../ops/config/README.md#定时备份策略)。手动 create/verify/restore 的参数与 stdout 保持原契约。

archive 命名为 `scheduled-<UTC秒时间>-<run_id>.dump`。本次新备份通过 verify 及已启用的副本阶段后，保留它和最近 N−1 份符合资格的旧备份：旧备份须在同一输出目录、有匹配库名摘要的 `completed` 回执、命名与运行 ID 一致、曾通过校验，且设备号/inode/大小与回执一致。文件必须是当前账号所有的 owner-only 普通单链接文件。删除前只复核这些证据和文件元数据，不重新读取旧 archive 或执行 verify；先删 manifest，再删 archive。本地校验前失败的运行回执（无 archive 或 verified 为 false）静默忽略，不记副本异常；已通过本地校验但副本资格不足时才记 `replica_not_verified`，副本证据不可读或无效时记 `replica_evidence_invalid`。实际目录项异常或无完成证明时仍记入 `cleanup.skipped` 并保留；列表最多记录 256 项，另有完整计数。手动备份和不完整备份不会自动删除。该次启用了异机副本时还要求 daily 完成证据；旧 v1 回执缺省为未启用，保持原清理资格。旧 failed/interrupted 项重试成功时追加 `offsite/completed/<run_id>.json`（`auction-backup-offsite.v1`），绑定原库名、输出目录和明文 manifest 摘要；清理可以使用这个事实，但不改写原运行终态。`offsite-staging/` 子目录不进入清理枚举或跳过噪音。

异机副本启用前必须完成[ops/deploy/RECOVERY.md#异机副本前置准备](../../ops/deploy/RECOVERY.md#异机副本前置准备)。`age --version` 预检失败发生在 pg_dump 之前。verify 后先在 `<state-dir>/offsite/pending/<run_id>.json` 保存任务身份，再分别用 `age --encrypt --recipient … --output <私有临时文件> <输入>` 加密 archive 和 manifest，stdin 关闭，stderr 有界捕获且不进入日志或回执，子进程取消时终止。密文位于同文件系统的 `<output-dir>/offsite-staging/<run_id>/<archive>.age`、`<archive>.manifest.age`；0600 临时文件 fsync 后无覆盖封存。同一任务永远复用原密文；封存后、checkpoint 前中断也会认领原字节，不重新加密。加密前预留明文大小、每 64 KiB 的 16 字节标签及每文件 16 KiB 头部余量，已暂存数据直接计入文件系统占用。

本次新副本优先处理，然后按开始时间重试旧待上传项；有限预算优先取得最新恢复点，不被积压饿死。重试共用总预算，检查目的地、公钥绑定、文件身份和摘要。无效、不可读、原回执不匹配或目的地已变更的旧任务跳过并保留原文件和密文，在 `offsite.skipped_pending` 记录固定原因码告警；这些跳过告警本身不使本次失败。单个项目上传、冲突或收口失败记录到 `offsite.failed_items`，继续其他项目；全部处理完或总预算耗尽时，只要有失败，本次 outcome 为 failed、退出 3，failure 保留首个失败的阶段与固定错误码。正在执行时 outcome 保持 running，预算耗尽将活动项目记为 timeout，且不覆盖更早的失败；状态目录整体无法枚举或运行记录无法写入时仍停止推进。任务持久保存原密文身份、摘要、已校验对象及固定字节的完成记录；上传失败退出 3，保留本地备份、密文和任务。daily 和所需 monthly 完成后追加本地完成事实，删除暂存密文与待上传项。长期失败会使本地积累直到 `disk_low`，不通过删除尚未取得副本资格的备份腾空间。

启用备份范围时，同一 daily/monthly 前缀下还会上传 `<archive>.scope.bundle.age` 与 `<archive>.scope.json`（`offsite.daily/monthly.scope`）：范围密文已在本地封存，不再重新加密；sidecar 只含摘要与相对路径，最后上传。旧待上传项没有范围对象时照常完成。对象路径为 `<prefix>/daily/YYYY/MM/DD/<run_id>/<文件名>`，日期取 manifest `created_at` 的 UTC 日期。每个目的地、每个备份 UTC 月的第一份完成副本还把相同密文写入 `monthly/YYYY/MM/<run_id>/`。月标记保存在 `<state-dir>/offsite/months/`；缺失时遍历 LIST 的全部月完成记录，格式、字段、缺件或长度/摘要无效的逐条跳过，只输出固定原因码 `monthly_record_invalid`。遇到首份完整有效记录即恢复标记；全部无效则正常创建本次月副本，LIST/GET 传输或权限错误仍使该项目失败。旧任务先前的未完成月选择会复核，避免新副本已完成后再次发布另一份月完成记录。先上传 archive、再 manifest，最后发布 `completed.json`（`auction-backup-upload.v1`：run_id、created_at、两份对象名/长度/密文 SHA-256/状态、明文 manifest SHA-256、age 版本、校验时间）；没有完成记录的副本不是成功恢复点。单次 PUT 原生 create-only；超过 128 MiB 用 64 MiB 起的分片、最多 10000 parts、并发最多 2。分片先 HEAD，其完成请求没有条件写入，依靠 bucket lock、唯一命名、原密文重用与完整校验保护不可变性。409/412 或响应丢失后，已有对象长度和完整 SHA-256 一致记 `already_present`，不一致为冲突；不覆盖、删除或换名。所有对象均 HEAD + 顺序区间回读校验；每个 get_range 不超过 64 MiB，累计实际长度和全部字节的 SHA-256，两个结果均匹配才承认成功。每段受 300 秒请求超时约束，整体仍受本次运行预算约束，不把 ETag 当摘要；工具不删除远端对象，淘汰由 bucket 生命周期完成。

磁盘使用备份目录文件系统的非特权可用块计算：字节与比例下限都须满足，任一次不足记录 `disk_low` 并退出 3，不先清理仍应保留的恢复点。总预算从命令启动计算，异步子进程、数据库等待、分块摘要读取和清理共享期限；到期记录 `timeout` 并退出 3；若此前已有项目失败，顶层 failure 保留首错，超时活动项目另记在 `offsite.failed_items`。终态同步及最多五秒的打卡另留 systemd 余量。`SIGKILL`、断电或 systemd 强制终止当次可能只留下运行中记录，故当次依赖 systemd 失败状态与 journal；下次启动取得同一状态目录锁后，把有效残留记录收口为 `interrupted`。若终态已发布、身份与已知 archive 证据一致，则沿用已有终态，不改写结果。残留记录无法解析、字段无效，或同名终态无效/冲突时，将原文件以 0600、无覆盖改名为 `<state-dir>/running.invalid-<UTC时间戳>-<本次run_id>.json`，记录固定告警码后照常备份；历史终态不修改。只有记录读写或改名保留等存储操作失败才记 `recovery_failed` 并停止。

终态文件为 `<state-dir>/receipts/<run_id>.json`，格式 `auction-backup-run.v1`，0600 临时文件写入并 fsync 后以 `RENAME_NOREPLACE` 无覆盖发布，再同步目录；历史终态不覆盖。运行中状态只有 `<state-dir>/running.json`，阶段更新以临时文件原子替换；`<state-dir>/scheduled.lock` 从初始化收口到打卡结束始终由当前进程持有，无单独记录租约。运行记录初始化先于配置加载，配置失败也写终态；状态目录本身无法创建或写入时依靠退出码和日志处理。

| 回执字段 | 含义 |
| --- | --- |
| `format/run_id/started_at/finished_at/elapsed_millis` | 格式、身份、UTC 时间与本次运行耗时 |
| `stage_timings` | 已结束阶段的单调时钟耗时；interrupted 保留最后 checkpoint 的已知值，不把停机时间补算成执行时间，finished_at 为收口时间 |
| `outcome/stage/failure` | 终态 `completed/failed/interrupted`；stage 是最后执行阶段，failure 保存首个失败的阶段与固定 `error_code`，不含底层错误链 |
| `database_name_sha256/output_dir/policy` | 库名绑定、受控输出目录、本次普通策略；配置尚未成功时 policy 可为空 |
| `archive` | 文件名、摘要、schema、创建时间、archive/manifest 的本机文件身份和 `verified`；恢复仍使用原 manifest v1 |
| `archive.scope` | 启用备份范围时的 sidecar/密文文件名、摘要、本机文件身份与文件数；旧 v1 回执缺省为空；不记录绝对来源目录或内容 |
| `disk_before/disk_after` | 检查时的总字节、可用字节、`reserved_bytes`（待写密文空间，旧 v1 默认 0）、阈值与是否通过；扣除预留后仍须满足两个水位 |
| `cleanup/recovered_interrupted` | 成功清理及跳过项目和计数、此次收口的中断数；不删除历史回执 |
| `recovery_warnings` | 旧记录无效时为 `running_record_invalid`，同名终态无效或冲突时为 `terminal_receipt_conflict`；只保存固定原因码，不包含文件内容，也不改变新备份结果；旧 v1 回执缺省为 `[]` |
| `heartbeat_configured` | 本次结果对应的成功或失败地址是否配置；不记录 URL 或发送结果，结果只写 stderr/systemd journal |
| `offsite` | `enabled/age_version`、`daily/monthly`（archive/manifest/completion 的 key、bytes、sha256、uploaded/already_present 及 verified_at）、`monthly_written`、`pending_objects`、`retried_runs/retried_count`；重试 ID 列表最多 256 项；旧 v1 缺省未启用 |
| `offsite.skipped_pending/skipped_pending_count` | 跳过旧任务的可选 run_id 与固定 reason；列表最多 256 项，另有完整计数；原因见下文，原任务与密文不改写、不删除 |
| `offsite.failed_items/failed_count/active_run_id` | 项目失败的 run_id、stage、error_code；列表最多 256 项，另有完整计数；active_run_id 为执行中的任务，正常终态为空，旧 v1 字段缺省兼容 |

`offsite.skipped_pending.reason` 为 `pending_unreadable`、`pending_record_invalid`、`pending_destination_changed`、`pending_identity_mismatch`、`pending_source_invalid` 或 `pending_source_mismatch`。告警只含固定原因码与可确认的 UUID，不记录文件内容、秘密或完整路径。当前 daily 即使已经成功，旧项失败仍会使整体 outcome 为 failed；选恢复点应核对 daily 完成证据和远端 completed.json。

常见错误码为 `config_error`、`output_lock_busy_or_invalid`、`output_directory_invalid`、`disk_check_failed`、`disk_low`、`create_failed`、`scope_failed`、`verify_failed`、`retention_failed`、`recovery_failed`、`receipt_failed`、`timeout`、`interrupted`。范围阶段失败时 archive 与 manifest 已发布但不带 sidecar，不能作为完整恢复点；按 `scope_failed` 排查来源权限、age、上限或文件变化后重跑。正常收口的成功/失败均输出一个 JSON 摘要（`outcome/run_id/receipt/failure`），调用方结合退出码判断；状态目录取锁失败不写回执，回执发布失败可能只保留运行中记录。

异机阶段为 `offsite_preflight/offsite_encrypt/offsite_retry/offsite_daily/offsite_monthly/offsite_finalize`，阶段错误码为 `offsite_preflight_failed/offsite_encryption_failed/offsite_retry_failed/offsite_daily_failed/offsite_monthly_failed/offsite_state_failed`；R2 适配器进一步给出固定分类 `offsite_configuration_invalid/offsite_local_read_failed/offsite_access_denied/offsite_transport_failed/offsite_object_conflict/offsite_remote_record_invalid`。底层 SDK 正文、凭据、签名 URL 和 age stderr 均不输出。回退为 `[backup.offsite] enabled = false`，下一次运行生效，暂停旧待上传项但不放宽其清理资格；停用 timer 停止后续全部定时任务。

打卡读取 Worker 敏感 TOML 的 `backup_heartbeat_success_url/backup_heartbeat_failure_url`，默认空即不建客户端、不联网。外部服务目前暂缓启用，示例保持空值。启用后，在终态发布后向对应地址发送一次 HTTPS GET，禁止 userinfo、重定向、环境代理与重试；连接预算三秒、请求总预算五秒，不携带 Cookie，不读取响应正文。结果只经 `progress()` 向 stderr 输出 `sent/failed/not_configured` 和可选 HTTP 状态码，systemd 下进入 journal，不另写打卡回执，不改变备份结果与退出码。Worker 配置尚不能加载时按未配置处理；回执存储失败则记录错误并尝试一次失败打卡。

### WAL 归档、每周基础备份与上传

```bash
# PostgreSQL archive_command 入口（以 postgres 账号执行，不读敏感 TOML）
auction-cli wal archive --staging-dir /var/lib/auction/wal-staging \
  --recipient age1<收件人公钥> /path/to/pg_wal/000000010000000000000001 000000010000000000000001

# 每周物理基础备份（以 postgres 账号执行，需要 REPLICATION 权限）
auction-cli wal base-backup --staging-dir /var/lib/auction/wal-staging \
  --recipient age1<收件人公钥>

# 独立上传器（auction 账号，systemd timer 每 5 分钟调用）
auction-cli --config-dir /etc/auction wal upload \
  --staging-dir /var/lib/auction/wal-staging --budget-seconds 300
```

`wal archive` 的 `SOURCE`（PostgreSQL `%p`）与 `NAME`（`%f`）都是位置参数，`NAME` 必须等于 `SOURCE` 的文件名；段名只接受 24 位十六进制 WAL 段、`<8 位十六进制>.history` 与 `<24 位>.backup`。命令校验收件人公钥形状（62 字节 `age1` + bech32）、暂存目录（绝对路径、非符号链接、禁止 other 写入）与源文件（普通文件）。随后按 `zstd -q -T1 -c -- <源>` → `age --encrypt --recipient <公钥>` 的顺序压缩与加密，写入 `<段名>.zst.age` 临时文件；两个子进程都退出 0 后才 `fsync` 文件、设为 0640、改名发布，再 `fsync` 目录并写入 `<段名>.zst.age.manifest.json`（`auction-wal-archive.v1`：段名、源字节数与 mtime、密文长度、归档时刻）。任一步失败都以退出码 3 结束并清理本次临时文件，由 PostgreSQL 重试同一段；失败原因只含固定阶段与有界 stderr 文本，不含密钥。

重复归档只认 manifest：同一段已有 manifest 且源字节数与 mtime 一致时输出 `already_archived` 并退出 0，不重复加密；只有密文而没有 manifest 表示上次在发布前中断，删除该密文后重新归档（PostgreSQL 从未收到成功退出）；manifest 存在而密文缺失、源内容不同或密文长度与 manifest 不符时失败关闭，不覆盖既有证据。

`wal base-backup` 生成每周物理基础备份，按 postgres 账号运行且不读敏感 TOML。它执行 `pg_basebackup --pgdata=- --format=tar --wal-method=fetch --checkpoint=fast --label <备份 ID> --no-password`，连接使用 libpq 默认值（本机 socket、peer 认证），因此执行账号必须有 `REPLICATION` 权限或为超级用户；`-D -` 要求集群没有额外表空间，`-X fetch` 把所需 WAL 并入同一个 `base.tar` 流。tar 流经 `zstd -q -T1 -c` 与 `age --encrypt --recipient` 写入 `<备份 ID>.tar.zst.age`（0640），`fsync` 文件与目录后发布 `<备份 ID>.tar.zst.age.manifest.json`（`auction-base-backup.v1`：备份 ID、开始/结束时刻、压缩前后字节数、密文 SHA-256）。备份 ID 固定为 `base-YYYYMMDDTHHMMSSZ`（UTC）；任一外部程序非零退出都以退出码 3 结束并清理临时文件，不发布任何一方；同一秒内重跑返回 `already_created`，只有密文而没有 manifest 时清理后重新生成。

`wal upload` 读取 Worker 配置，`[backup.offsite] enabled = false` 时执行本地基础备份保留清理，输出 `outcome = skipped` 并退出 0（清理失败时为 `failed`）；否则在暂存目录内取 `wal-upload.lock` 互斥锁，按段名顺序把 `<段名>.zst.age` 上传到 `<prefix>/wal/<段名>.zst.age`，再按备份 ID 顺序把每组基础备份上传到 `<prefix>/base/<备份 ID>/`：先密文、再 manifest，最后发布字节固定的 `completed.json`（`auction-base-upload.v1`）。远端已有相同长度与完整 SHA-256 的对象记 `already_present`（bucket lock 下重传返回 409）；WAL 校验通过后先写 `<段名>.wal-upload.json` 持久证据（源字节/mtime、密文长度与 SHA-256、远端键），再删除本地密文与 manifest 并 `fsync` 目录；PostgreSQL 重试同一段时据此判定已归档，避免重新加密后摘要不一致。基础备份成功上传后写入本机证据 `<备份 ID>.upload.json`（`auction-base-receipt.v1`，含目标身份），只有 endpoint、bucket 与前缀仍一致时后续运行才据此跳过重复回读（`base.already_verified`），证据缺失、无效或目标变化按未上传处理；本地保留最新 3 组完整暂存项，更旧的组依次删除本机证据、manifest 与密文，本次上传失败的组不参与删除。失败、冲突或预算耗尽保留本地文件并以退出码 3 结束。stdout 报告含 WAL 的 `uploaded/already_present/failed/invalid/incomplete/remaining` 与 `base.uploaded/base.already_present/base.already_verified/base.failed/base.retained/base.removed`：`invalid` 是 manifest 无效或长度不符的项，`incomplete` 是创建进程尚未发布完成的项（WAL 段名或基础备份 ID），两者都不删除本地文件。

归档密文是 age 公钥加密结果、manifest 无秘密，因此发布为 0640 并依赖暂存目录的 setgid 组共享（`postgres:auction`、`2770`）；上传器与归档命令使用不同账号时不要改成 0700。启用顺序、停机时段门禁、验证查询与时间点恢复见[ops/deploy/RECOVERY.md#wal-归档与时间点恢复](../../ops/deploy/RECOVERY.md#wal-归档与时间点恢复)。`archive_mode` 默认仍为 `off`；本仓库尚未在隔离实例演练时间点恢复。

### 主机侧运维告警报告

```bash
auction-cli --config-dir /etc/auction alert report   --backup-output-dir /var/backups/auction/scheduled   --backup-state-dir /var/lib/auction/backup   --wal-staging-dir /var/lib/auction/wal-staging   --restore-drill-dir /var/lib/auction/restore-drills
```

`alert report` 读取 Worker 配置（含 `[backup]` 水位与 `[backup.offsite]`）后采集三项主机事实：备份输出目录的 `statvfs` 非特权可用块、`<state-dir>/receipts/` 中开始时间最晚的终态回执、WAL 暂存目录中已发布 manifest 且尚未上传的段。它不执行备份、上传或迁移，也不写回执；报告只输出安全投影（kind/scope/count/age/fingerprint/摘要），不含路径、底层错误正文或凭据。

- `backup` 来源：可用空间不同时满足 `[backup].min_free_bytes` 与 `min_free_percent` 时产生 `disk_capacity_low`（count 为缺口字节）；最近一次终态为 `failed`/`interrupted` 时产生 `backup_failed`（age 为距 `finished_at` 的秒数）。目录或回执不可读、或没有任何终态回执时，观察标记为来源不完整（`source_unavailable`），已确认的异常仍然上报，但不会关闭该来源的历史告警。
- `backup` 来源的恢复演练（决策 31，可选 `--restore-drill-dir`）：只读该目录一层的 `*.json`，只认成功的 `auction-postgres-restore.v1`（取 `restored_at`）与 `conclusion.result` 为 `passed` 的 `auction-migration-drill.v1`（取 `finished_at`），跳过符号链接、超过 1 MiB 与无法解析的文件。最新成功演练超过 35 天或没有任何成功回执时产生摘要级 `restore_drill_overdue`（从未演练时 count 为 0）；目录不可读时来源标记为不完整、不关闭历史告警；省略参数则不检查，已有的 `restore_drill_overdue` 保持原状（不会因本次缺席被关闭），同一报告中的磁盘与备份告警照常按结果关闭。
- `wal` 来源：`[backup.offsite] enabled = false` 时暂存是本机唯一副本，报告输出 `wal_offsite_disabled` 并解除历史告警；启用异机副本时，最老暂存段达到 30 分钟产生 `wal_upload_stalled`（count 为暂存段数，age 为最老段年龄）。暂存目录不可读时同样标记来源不完整。
- 未启用 `[worker.operations_alert]`（或告警运行状态为停用）时输出 `outcome = skipped`、`reason = operations_alert_disabled` 并退出 0，不写任何告警行。
- 幂等：signal fingerprint 只由 `kind` 与固定 scope 决定，重复运行是 upsert；`next_notify_at` 在已有行上保持不变，因此 CLI 重试不会产生第二条告警或重置冷却。`backup` 与 `wal` 独立提交，缺少一项证据不影响另一项来源的恢复判定。
- 固定观察窗口（代码常量，不进入运行配置）：WAL 暂存积压 30 分钟。timer 示例见 `ops/deploy/systemd/auction-alert-report.{service,timer}.example`；处置步骤见[ops/README.md#告警处置手册](../../ops/README.md#告警处置手册)。

## 业务运维

以下命令都需要 `postgres` feature，读取 Worker 配置。

### 凭据

```bash
auction-cli credential list --kind mail --status active
printf '%s' "$COOKIE" | auction-cli credential import --kind mail --world main --label bot-main --actor ops
```

`import` 只从 stdin 读取 Cookie 头（去掉末尾换行），用敏感配置中的 `credential_data_key` 加密后写入，与 Admin 凭据页共用同一服务、命令账本与审计。两种凭据都必须含合法的 `ipb_member_id` 与 `ipb_pass_hash`：`--kind mail`（MoogleMail 游戏 Cookie）只保存这两个字段；`--kind forum`（E-Hentai 论坛 Cookie）保留通过校验的原始 Cookie 头。`--world shared` 同时服务 main 与 isekai。Worker 在启动时装载 active 凭据，导入后需重启 Worker 生效。`list` 只输出安全投影，不含密文或明文。

### 规则导入

```bash
auction-cli rules import --directory /srv/rules --command-id 6f1d…-… --actor ops
```

目录须含 `en.md`、`zh-CN.md`、`zh-TW.md`、`ja.md`、`ko.md` 五个 UTF-8 普通文件：去掉开头单个 BOM，首个非空行去掉前导 `#` 作为标题，正文按 Admin 规则校验。写入前先比对数据库现状：任一语言已存在不同正文即整体拒绝；内容相同报告 `preserved`；缺失语言以 `--command-id` 派生的稳定命令 ID 发布（请求摘要与 Admin 发布接口一致），部分完成后用同一 `--command-id` 重试即可续上。规则发布直接写入数据库，Public 每次读取都回源数据库。

### 论坛写入恢复

```bash
auction-cli forum recovery --limit 100
auction-cli forum authorize-retry --auction-id <拍卖UUID> --command-id <UUID> --actor ops \
  --confirm 'AUTHORIZE FORUM WRITE <拍卖UUID>'
```

`recovery` 只读列出主题写入结果未知、只读对账中或等待人工授权的拍卖。Worker 对未知写入先按精确标题只读查找主题；未命中时停在 `write_authorization_required`，此时才可用 `authorize-retry` 以稳定命令 ID、逐字确认与追加审计授权下一次创建尝试。同一命令 ID 换参数即冲突。

### 装备 PM 对账

```bash
auction-cli pm list --status manual_reconcile
auction-cli pm reconcile --notification-id <UUID> --command-id <UUID> --expected-fencing-version <N> --actor ops
```

`list` 输出超价提醒私信的安全状态、fencing 版本、时间与原发送者，不含正文、收件人或租约 token。`reconcile` 以版本 CAS、稳定命令与追加审计请求 Worker 用原发送者只读核对 Sent；返回的是请求被接受的快照，不表示私信已送达。没有重发、放弃或强制成功入口。

### 邮件证据

```bash
auction-cli mail export-evidence <evidence-id> --output /secure/evidence-<id>.json
auction-cli mail inspect-sent <obligation-id> --output /secure/sent-<id>.json
```

- `export-evidence` 按 Worker 日志中的 `evidence_id` 解密导出 MoogleMail 诊断归档（目录为 `[worker.mooglemail.diagnostics].directory`）。归档头部的 `key_source` 决定使用哪个密钥域的密钥环（`mail_diagnostics`、`credential` 或 `equipment`），可用 `--key-source` 限定候选域；同一域内保留的旧 key 也能解密历史证据。不联网、不写数据库。
- `inspect-sent` 只针对撤拍退回（`withdrawn_return`）义务：读取冻结的收件人、世界与装备，用该世界唯一的 active 邮件凭据只读查询 Sent 中的精确邮件，结果标记 `time_unverified`，`collected` 也不等于已送达证明。不发送、不改变履约状态。

### 装备损失人工收口

```bash
# 默认只读，不访问游戏或论坛
auction-cli --config-dir /etc/auction settlement equipment-loss <LOT_UUID>

# 逐字使用 preview 的版本、账户、卖方金额与 confirmation，先替换全部占位值
auction-cli --config-dir /etc/auction settlement equipment-loss <LOT_UUID> --execute \
  --command-id <STABLE_COMMAND_UUID> --actor ops --reason '<装备损失原因及人工处置责任>' \
  --funding-uid <UID> --expected-recovery-version <RECOVERY_V> \
  --expected-payment-version <PAYMENT_V> --expected-delivery-fencing-version <DELIVERY_V> \
  --seller-amount <SELLER_AMOUNT> --buyer-compensation-external \
  --confirm 'RESOLVE EQUIPMENT LOSS <LOT_UUID> <UID> <RECOVERY_V> <PAYMENT_V> <DELIVERY_V> <SELLER_AMOUNT> EXTERNAL BUYER COMPENSATION'
```

- 默认输出 `outcome=preview`，`preview` 原样序列化应用端口的安全投影，包含 `eligible`、`blockers`、资金 UID、恢复/付款履约/投递版本与冻结买卖方金额。`execution.confirm` 给出逐字确认短语，`execution.argv_template` 给出独立参数数组模板；替换配置目录、稳定命令 UUID、操作者与原因占位值后按参数执行，不要把 JSON 直接交给 shell `eval`。没有可投影的资金 UID 时 `execution=null`；有模板不代表资格通过。
- 执行必须同时提供示例中的全部参数。缺少 `--execute` 时不得携带执行参数；缺参数、零 UUID、操作者格式错误或确认短语不匹配时在写入前拒绝。全部业务资格、资金模式与状态版本由结算 owner 在事务中校验；CLI 不另查 SQL 或推断 Sent。
- 本命令仅用于 main 已结算成交、原装备 COD 因库存读取失败而停在人工核对、全部发送尝试明确未开始外写的装备损失。只支持已就绪的 `record_only` 资金账户，`strict_cash` 拒绝执行；账户模式与既有结算金额均不改变。任何外写未知、既有收款或冲突恢复/资产处置都不能靠该命令绕过。
- `--buyer-compensation-external` 是操作者承担买方线下补偿责任的声明，不能证明线下补偿已支付。原付款以 `operator_equipment_loss_v1` 人工处置证明收口，绝不伪造 Sent、COD 收款或卖方已收款；责任资金沿原冻结结算分配，不记为真实 `collected_payment`。原结算与补偿流水保留，不走成交撤销冲正、重列或拉黑。
- 成功输出 `outcome=committed`，同一 ID 同参数重放输出 `outcome=replayed`，`result` 包含原付款投递、卖方义务与金额等审计结果。连接或响应错误不能证明未提交：必须原样重放同一命令 ID 与全部参数，不要先用新 preview 的版本替换原请求；同 ID 换参数冲突。
- 原装备投递永久终结为 `failed/equipment_destroyed`，不能再从 Admin 重试或恢复投递；不再催款或执行该件买方退款。卖方货款只解除阻塞，仍使用原冻结金额，通过正常 `SellerPayout` 履约和真实邮件支付，命令成功不代表卖方已收款。详见[docs/guide/settlement.md#装备损失人工处置](../../docs/guide/settlement.md#装备损失人工处置)。

### Sent 人工证据确认

```bash
# 只读数据库预览；证据由操作者观察原生 Sent 后准备，不发起游戏请求
auction-cli --config-dir /etc/auction settlement confirm-sent <DELIVERY_UUID> \
  --evidence /private/sent-evidence.json

# 逐字使用 preview 的版本、确认短语及要求的接受项
auction-cli --config-dir /etc/auction settlement confirm-sent <DELIVERY_UUID> \
  --evidence /private/sent-evidence.json --execute \
  --command-id <STABLE_COMMAND_UUID> --expected-fencing-version <FENCE> \
  --actor ops --reason '<原生证据来源及人工关联责任>' \
  --accept-collected-assets --accept-attempt-history --confirm '<PREVIEW_CONFIRM>'
```

- 证据只按闭集 `ManualSentEvidence` 解析：`schema_version=1`，`world_id`、`external_mail_id`、`sender_uid`、`sender_name`、`recipient`、`subject`、UTC `sent_minute`/`observed_at`、32 字节数组 `original_body_digest`/`observed_body_digest`、整数 `remaining_credits`/`cod_pending`、字符串数组 `equipment_ids`、布尔 `collected` 和原生 `receipt`。字段全部必填，未知字段拒绝。最多 64 KiB，使用既有有界、非阻塞普通文件读取器，拒绝符号链接、FIFO、设备等特殊文件；不要将凭据放进证据。
- `equipment_ids` 只记录远端当前实际剩余 EID，已领取时为空；不得将期待 EID 填入观察值。原始正文摘要绑定冻结发送正文，不能用当前模板重渲染；已领取时还须保留游戏追加的原生附件收取/COD 回执。应用端口统一核对冻结期待、发件身份、发送分钟区间、金额、附件与历史尝试。
- 入口支持 main 结算的单件装备 COD、流拍/撤拍退回，以及原货款已寄出的 `seller_payout`。原投递须有冻结发送尝试、处于结果未知的人工核对状态或被 Admin 对账判失败且其后无变化（预览 `corrects_admin_failure=true`），且无活跃 claim 或冲突处置。货款只接受正额 Credits、零 COD 与空 EID：未领取时原额仍在且回执为空，已领取时须带原生 `Removed attachment: <千分位金额> Credits` 回执并加 `--accept-collected-assets`；原 `seller/unknown` 资金占用及 reserve/unknown 分录必须精确绑定同一账户、lot、义务、投递和金额。默认 `outcome=preview`，`preview.blockers`/`eligible` 表示当前资格；预览不写入或授权重发。
- 执行参数只能与 `--execute` 同时使用；命令 ID、版本、操作者、原因与 `--confirm` 全部必填。`--accept-collected-assets` 和 `--accept-attempt-history` 只在 preview 要求时加入，示例中的两项不能机械照抄。确认短语绑定投递、版本、MID、类型化证据摘要与两项接受声明，任何内容变化均须重新预览。`execution.argv_template` 是独立参数数组，不得 `eval`。
- 已收取附件后原 EID 无法再自动证明，接受项记录操作者承担的资产关联责任；旧尝试历史不完整时，另一接受项记录人工关联责任。系统保留全部旧尝试、原结果与审计，追加独立不可变确认，以 `sent_operator_confirmed_v1` 区别于 `sent_exact_payload_v1` 自动精确资产证明，不设置 `sent_verified=true` 冒充自动核验。Admin 仍不能仅凭手填 MID 确认主动 Send 成功。
- 成功输出 `outcome=committed`，同 ID 同参数重放输出 `outcome=replayed`。响应或连接错误不能证明未提交，必须保持原命令 ID、证据内容、原因、版本、接受项与确认短语重放；不要用新 preview 的版本替换原请求。同 ID 换参冲突。
- 装备确认只绑定真实 MID、恢复原投递及履约；COD 实收及后续卖方款仍由 Worker 使用真实收取证明与原冻结结算处理。已寄出的货款确认则复用资金 owner，将原 `unknown` 支出记为 `spent`，不创建新付款、收款来源或投递；同命令重放不重复支出。确认均不代表收件人已经领取，领取状态由正常流程继续核验。更正 Admin 失败结论时原失败命令与审计保留，履约从 failed 恢复正常对账；邮件早于当前尝试时须加 `--accept-attempt-history`。详见[docs/guide/settlement.md#sent-人工证据确认](../../docs/guide/settlement.md#sent-人工证据确认)。

### Bot 资金

```bash
auction-cli funds preview --uid <UID>
auction-cli funds open-record-only --uid <UID> --command-id <UUID> --actor ops \
  --expected-version 0 --preview-digest <HEX> --confirm '<preview 给出的 open_record_only 模板>'
auction-cli funds reconcile --uid <UID> --command-id <UUID> --actor ops --expected-version <V> \
  --preview-digest <HEX> --proprietary-credits <P> --compensation-credits <C> --dedicated-account \
  --confirm 'RECONCILE BOT FUNDS <UID> <V> <P> <C> <HEX>'
```

Bot 内部履约（买卖双方之一是 Bot 自己时不经邮件外发）需要一个资金账户，见[docs/guide/settlement.md#bot-内部履约与资金账户](../../docs/guide/settlement.md#bot-内部履约与资金账户)。

- `preview` 只读数据库，输出账户版本、责任投影、阻断项、`preview_digest`、能否开 record_only 账户，以及两种写入的确认短语模板；不访问远端，不代表任何可用额度。
- `open-record-only`：全局尚无资金账户时，建立只记账（`record_only`）账户——系统记录结算责任与真实收款，不读取余额、不以 Bot 现金限制个人消费。已有账户不能用此命令切换模式。
- `reconcile`：为 `strict_cash` 账户做现金对账，声明本次授权的自有额度与补偿额度；须声明 `--dedicated-account`（专用账号、无并发人工资金写入、无未登记外部负债、全部程序同版）。对账后账户状态不是 `ready`（账实缺口，新增支出冻结）时以 5 退出。
- 写入都先按原命令 ID 重放；之后在 30 秒共享预算内取得与 Worker 相同的邮件/论坛凭据 owner，用 Main 世界唯一的 active 邮件凭据与论坛凭据完成联合身份、干净草稿（以及 strict 模式的现金）观察，再在同一 opaque guard 内提交。因此**必须先停止 Worker**，并启用 `[worker.forum]` 与 `credential_data_key`。
- 输出 `committed`/`replayed` 才算完成；`rolled_back` 为确定回滚；`evidence_unavailable` 表示提交前证据未通过；`outcome_unknown` 表示提交阶段结果未知——保留原命令 ID 与全部参数重新执行，以重放核对，不要换 ID 重试。

凭据、规则与资金命令在连接池建立后，将参数校验、客户端装配与执行错误统一带回连接池关闭路径；结算损失命令在连接前校验执行声明与确认短语，端口读取、提交、序列化和输出错误也统一带回关闭路径。退出前显式关闭连接池；规则文件复用证据文件的非阻塞普通文件检查与有界读取。

### 恢复后外部核对

```bash
# 1. 先暂停全部外部访问（带期限；到期自动恢复并留审计）
auction-cli --config-dir /etc/auction switch set external_access_paused true --until <RFC3339>

# 2. 只读演练：扫描 Sent / Read Mail / 论坛新楼层并输出计划回执
auction-cli --config-dir /etc/auction recovery external-reconcile   --cutoff <BACKUP_TIME_RFC3339> --receipt /secure/recovery-<time>.json

# 3. 确认回执后执行隔离；同一 scan ID 可重放
auction-cli --config-dir /etc/auction recovery external-reconcile   --cutoff <BACKUP_TIME_RFC3339> --receipt /secure/recovery-<time>-applied.json   --apply --confirm 'APPLY EXTERNAL RECONCILIATION' --actor ops   --scan-id <RECEIPT_SCAN_ID>
```

- 命令要求 Worker 已处于全局暂停；未暂停时 Worker control 返回 `409 not_paused`，请求不发往远端。扫描只读；除显式 `--apply` 的本地隔离外不执行 Send/Take/Return、不标记已读、不写远端。
- 第一阶段枚举 `--cutoff` 之后的 Sent 与 Read Mail 摘要（`--maximum-pages` 每世界上限）并读取最多 `--maximum-forum-threads` 个已发布主题；第二阶段只为列表投影匹配的候选 MID 读取详情。列表标题/收件人是截断投影，只有详情完整收件人、完整标题、发送时间与冻结发送预期（金额、COD、装备 EID；已领取时只按身份）全部一致才形成精确匹配。
- `--apply` 把当前仍为 `pending`/`claimed` 且 fence 未变的精确匹配投递改为 `manual_reconcile`，在同事务追加 `delivery_external_quarantines` 与审计；不重发、不领取、不改写资金或结算。重复扫描或同 `--scan-id` 重放幂等跳过。Admin 仍用既有 `manual_reconcile` 动作人工处置。
- 回执 `auction-external-recovery.v1`（0600、不覆盖）记录扫描完整性（`sent_complete`/`read_mail_complete`/`errors`）、候选/精确/歧义/资产不一致计数、计划与已隔离投递、恢复库缺少的已读入站 MID 及论坛新楼层。扫描不完整、Worker 在扫描期间解除暂停或详情读取失败时，回执如实标记；`未观察到` 不能证明未发送，恢复库缺少的邮件/帖子必须人工核对。
- 完成后按核对结果恢复外部访问；隔离只表示“备份后可能已寄出”，对应义务仍需 Admin/Worker 用真实 Sent 证据或人工证明收口。

## 源码结构

| 路径 | 职责 |
| --- | --- |
| `apps/auction-cli/src/main.rs` | 入口：平台限定、umask、退出码 |
| `apps/auction-cli/src/cli/` | 命令、参数与默认值 |
| `apps/auction-cli/src/app/` | 分派、退出码、JSON 输出 |
| `apps/auction-cli/src/deployment/` | 配置目录、单角色投影、三角色一致性检查、监听端点 |
| `apps/auction-cli/src/service/` | systemctl 客户端、readiness 探针、编排与停机门禁 |
| `apps/auction-cli/src/deploy/` | 制品静态校验、迁移演练回执、在途发送门禁、停机备份双 CLI 校验、旧程序保留/原子安装、`deploy`/`rollback` 编排 |
| `apps/auction-cli/src/database/` | 状态、迁移、建库、预检/盘点证据、重建、连接目标解析 |
| `apps/auction-cli/src/backup/` | manifest、libpq 环境与 passfile、可复用 create、备份/校验/恢复 |
| `apps/auction-cli/src/backup/scheduled/` | 定时编排、目录边界、磁盘门禁、独占锁/回执、保留清理与打卡装配 |
| `apps/auction-cli/src/backup/scheduled/offsite/` | age、同盘暂存、待上传检查点、日/月副本编排与追加完成事实 |
| `apps/auction-cli/src/backup/wal/` | WAL 归档与每周 `pg_basebackup`（zstd+age+fsync 暂存）、暂存 manifest 与重复判定、R2 `wal/`+`base/` 上传器与本地保留 |
| `apps/auction-cli/src/operations/` | 凭据、规则、论坛、PM、邮件、资金与结算异常 |
| `apps/auction-cli/src/support/` | 命令共用的本地边界，不含业务规则：`files.rs` 负责私有目录（绝对路径、当前账号所有、禁止 group/other）、`O_NOFOLLOW` 锁文件、有界普通文件读取与 `create_new`+`0600` 原子发布；`process.rs` 负责子进程有界 stderr 捕获、退出状态描述与暂存/临时文件清理；`text.rs` 负责操作者、命令 ID、确认短语与摘要等输入校验（`process.rs` 与 `text.rs` 随 `postgres` feature 编译） |

排空配置和外层预算见[ops/config/README.md#排空与停止预算](../../ops/config/README.md#排空与停止预算)。systemctl 停止前只读取指定的 TimeoutStopUSec 属性，拒绝无限或无法解析的时长；不会把客户端超时视为 unit 已停止。


## 运行时开关

`auction-cli --config-dir /etc/auction switch list` 只读返回数据库时间、
七个已知名称的最新事实和 revision（未设置为 0），不证明运行进程已应用。
set/clear 直接执行在线命令，不停机、不调用停机时段门禁、不隐式迁移，
也不要求先停止 Worker。

```bash
auction-cli --config-dir /etc/auction switch set readiness_audit_failure_mode classified \
  --command-id <稳定非零UUID> --expected-revision <list中的版本> \
  --expires-at 2026-12-01T00:00:00Z --actor <操作者说明> --reason <非敏感原因>
auction-cli --config-dir /etc/auction switch set readiness_cache_enabled false \
  --command-id <新的稳定UUID> --expected-revision <list中的版本> \
  --no-expiry --actor <操作者说明> --reason <非敏感原因>
auction-cli --config-dir /etc/auction switch clear readiness_audit_failure_mode \
  --command-id <新的稳定UUID> --expected-revision <list中的版本> \
  --actor <操作者说明> --reason <非敏感原因>
auction-cli --config-dir /etc/auction switch set mooglemail_poll_isolation_enabled false \
  --command-id <新的稳定UUID> --expected-revision <list中的版本> \
  --no-expiry --actor <操作者说明> --reason <非敏感原因>
auction-cli --config-dir /etc/auction switch set metrics_reject_forwarded_requests true \
  --command-id <新的稳定UUID> --expected-revision <list中的版本> \
  --no-expiry --actor <操作者说明> --reason <非敏感原因>
```

set 的绝对 RFC3339 到期与 `--no-expiry` 必须二选一；期限最多六位小数、不接受闰秒，与 PostgreSQL 微秒精度一致。审计模式与告警读侧口径（`disposition_read_mode`）只接受 legacy/shadow/classified，缓存只接受 true/false。clear 不删除历史，只追加事实并回落各进程启动时的 TOML 基线。没有重读文件或重启；修改文件基线仍按原部署流程执行。

`metrics_reject_forwarded_requests` 是安全策略，只允许在线收紧：
`set true` 与 `clear` 被接受，`set false` 在命令校验层固定拒绝，
错误正文为“指标转发拒绝只能在线收紧；放宽须修改部署配置并重启”，
退出码仍是业务运维命令的 5。放宽必须修改 common 的
`[metrics] reject_forwarded_requests` 并重启对应角色。
邮件隔离开关切换在 Worker 下一轮调度或下一次维护门复读后生效；
在途 claim、租约、批次预算与 sweep 不因切换重置。告警读侧口径由 Admin/Worker
在每次读取告警快照前解析：缺省 `shadow` 继续显示旧口径的裁决值，只有把影子期差异
（`auction_shadow_mismatch_total{owner="disposition_reads"}`）观察到零并经用户批准后，
才 `set disposition_read_mode classified`；`legacy` 是回退路径。

命令返回 `status=database_accepted` 和固定 `accepted`，含义是“数据库已接受”。同 ID 同参重放原结果，即使已经过期或已有新命令也不续期、不覆盖。异参、旧 revision 被拒绝；重新读取后，新的意图使用新 ID。提交结果未知时必须使用原 ID 和全部原参数原样重试，不能换 ID 重发。`--actor` 当前仍是操作者声明，CLI 审计来源在 metadata 中标明 executor/entrypoint；自动 UID/SUDO_USER 追溯尚未接入。

成功退出 0；clap 参数错误退出 2；switch 子命令的配置加载、连接、读取、校验、版本冲突及结果未知均归业务运维命令退出 5。它不使用 service 的退出码 4，也不触发停机。刷新和三角色核对见[ops/README.md#运行时开关](../../ops/README.md#运行时开关)。

## 数据库连接用途

CLI 始终显式选择维护用途，读取 Worker 敏感目标不代表使用 Worker 服务档；池档位、超时与预算规则由[ops/config/README.md#数据库连接分档](../../ops/config/README.md#数据库连接分档)拥有。

入口与 application_name：

- db status：`auction-cli-status`。
- db migrate、db grants、service start/restart 迁移、fresh-schema 重建后的迁移：
  `auction-cli-migrate`（连接使用 `--owner-secrets` 的迁移 owner，不是服务账号）。
- db preflight / inventory / retention-dry-run / retention-purge：`auction-cli-preflight` / `auction-cli-inventory` / `auction-cli-retention-dry-run` / `auction-cli-retention-purge`。
- backup create / scheduled 的前后 SQL 核对：`auction-cli-backup-check`；
  外部 pg_dump：`auction-cli-backup`。
- backup restore 的前后核对及外部 pg_restore：`auction-cli-restore`（同样使用迁移 owner）；
  预检池先关闭，才执行外部恢复，之后另建核对池。
- db create / rebuild 的维护库直连：`auction-cli-create` / `auction-cli-rebuild`；
  建库后置目标库验证复用 create 用途。
- 凭据、论坛、邮件、资金、结算、规则、PM 和开关运维：`auction-cli-operations`。
- 停机窗口 inspect：`auction-cli-service-check`。
- backup verify / pg_restore --list 只读 archive，不连接数据库。
- 当前没有独立通用 backfill 命令；迁移内回填随 migrate 档执行。

源码同时借出峰值通常为一个池连接；建库为维护库与目标验证直连共 2，重建 fresh-schema 为 guard 与迁移连接共 2，备份为 SQL 核对池与 pg_dump 会话重叠。funds open-record-only/reconcile 另长期持有邮件与论坛 owner 共 2 个再加一个普通事务，代码借出峰值为 3，默认 4 尚有 1 个余量，未发现超过默认 4 的路径。物理会话保守上限另看空闲池：备份/重建可能达到池上限 4 再加独立会话 1，该额外会话由 external_reserved_connections 预算覆盖，不能声称总数硬限 4。

`deployment::check`（命令 `config check`）输出 database_budget 表，
汇总三角色实际上限、长期预留、CLI 并行和外部预留，profiled 超预算失败。
`config show` 仅增加非敏感配置投影；预算不连接数据库或核实 reserve。
字段和启用/回退顺序见[ops/config/README.md#数据库连接分档](../../ops/config/README.md#数据库连接分档)。


## SLI 汇总开关

Worker 专用 bool `metrics_sli_snapshot_enabled`，
CLI 沿用既有 set/clear、稳定命令身份、revision 和期限规则。
TOML 基线为 common 的 `metrics.sli_snapshot_enabled`，默认 false。
关闭汇总应明确 set false；clear 只回到当前基线，未必关闭。
完整启用、查看与回退见[ops/README.md#sli-基线与容量观察](../../ops/README.md#sli-基线与容量观察)。
