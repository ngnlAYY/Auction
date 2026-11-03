# 运维手册

[文档导航](../docs/README.md) · [配置参考](config/README.md) · [备份与恢复](deploy/RECOVERY.md) · [auction-cli](../apps/auction-cli/README.md) · [架构](../docs/architecture.md)

本手册覆盖生产部署、启停编排、升级、反向代理、监控与日常运维。常规运维通过 `auction-cli` 或 systemd 完成；普通服务启动从不隐式迁移、备份或删库。合并部署的首次设置在网页明确确认后，可由 `auction-server` 初始化预先建立的专用空库。

## 部署形态

| 形态 | 组成 | 适用 |
| --- | --- | --- |
| 分体部署 | `auction-worker`、`auction-admin-api`、`auction-public-api` 三个 systemd unit，`auction-stack.service` 调用 `auction-cli service` 按序编排 | 需要单独重启角色、细粒度资源限制或日后拆分主机 |
| 合并部署 | 一个 `auction-server.service`，以子进程运行三个角色 | 单主机、小规模、希望最少运维对象 |

两种形态共用同一配置布局与同一个 `auction-cli`，但不能同时用于同一套配置。

发布包（`auction-rust-<版本>-<提交前 12 位>[-single].tar.gz`）包含 `bin/`（分体包：三个角色程序 + `auction-cli`；合并包：`auction-server` + `auction-cli`）、`docs/`（部署、恢复、CLI 说明与 Web 资源摘要）、`systemd/`、`caddy/`、`config/examples/` 与 `retention-policy.v1.json`；分体包另含 `tmpfiles/`，合并包另含首次设置代理示例与单文件说明（以上均为发布包内的路径）。敏感模板以 `secrets.toml.example`、`secrets/postgres-rebuild.toml.example` 命名，须复制为配置引用的实际文件名。用发布包内的 `SHA256SUMS` 校验解压后的文件。

两种发布包均包含 `auction-backup.service.example` 与 `auction-backup.timer.example`；仅由用户按[定时备份](deploy/RECOVERY.md#定时备份)单独安装启用，不随三个角色服务自动开启。
两种发布包的 `docs/` 也包含 `MIGRATION-DRILL.md` 与 `migration-drill-receipt.template.json`，用于含迁移版本部署前的手工演练与留证。

启用异机副本还须安装主机 `age`、离线托管三组数据密钥及 age 私钥，并由用户在 R2 控制台配置限定 bucket 的令牌、生命周期与 bucket lock，见[异机副本前置准备](deploy/RECOVERY.md#异机副本前置准备)。

## 目录与账号

| 路径 | 内容 | 建议属主与权限 |
| --- | --- | --- |
| `/opt/auction/bin/` | 程序 | root，0755 |
| `/etc/auction/` | 配置目录：`public.toml`、`admin.toml`、`worker.toml`、`common.toml` | 目录 root:auction 0750；文件 root:auction 0640 |
| `/etc/auction/secrets.toml` | 敏感配置 | auction:auction 0600 |
| `/etc/auction/runtime/admin.toml` | runtime 配置（Admin 写入） | 目录 auction:auction 0750；文件 auction:auction **0640** |
| `/etc/auction/maintenance/postgres.toml` | 建库/重建维护凭据（可选，仅 `auction-cli` 使用） | root 0600 |
| `/run/hv-auction/` | Public/Admin 的 Unix socket 与实例锁（tmpfiles 预建） | auction:auction 0750 |
| `/run/hv-auction/worker/` | Worker control socket（Worker unit 的 `RuntimeDirectory`） | auction，0750 |
| `/var/lib/auction/` | 部署控制锁（`deployment/`）、邮件诊断归档等可写数据 | auction / root |

账号：

```bash
useradd --system --home-dir /var/lib/auction --shell /usr/sbin/nologin auction
groupadd --system auction-worker-control
usermod -aG auction-worker-control auction
```

`auction-cli config check --strict` 检查配置目录与普通配置文件不能被 group/other 写入、敏感文件只能 owner 访问；runtime 文件在 loader 阶段就必须恰好 0640 且父目录禁止他人写入。`service start/restart` 还会核对 runtime 文件与父目录归 Admin unit 的固定 `User` 所有。

> 生产环境中 `/etc` 对服务只读（`ProtectSystem=strict`），Worker 的邮件诊断归档默认目录（Worker 配置旁的 `mail-diagnostics/`）不可写。请在 `worker.toml` 把 `[worker.mooglemail.diagnostics] directory` 设为 `/var/lib/auction/mail-diagnostics` 这类可写的私有目录；runtime 配置不接管此路径。

## 首次部署

### 分体部署

1. **安装程序**：解压发布包，把 `bin/*` 安装到 `/opt/auction/bin/`。

2. **准备数据库账号与数据库**（PostgreSQL 18）：先建立应用角色；可手工 `CREATE ROLE … LOGIN PASSWORD …` / `CREATE DATABASE auction OWNER auction` 准备数据库，也可先按下一步手工写好配置，再用维护凭据创建缺失的库（`db create` 不创建应用角色，也不迁移）。首次设置向导会试连数据库，使用向导时须预先建好库：

   ```bash
   auction-cli --config-dir /etc/auction db create --database auction \
     --maintenance-secrets /etc/auction/maintenance/postgres.toml
   ```

3. **生成配置**，二选一：
   - 首次设置向导：先以 root 执行 `install -d -o auction -g auction -m 0750 /var/lib/auction`，确保目标目录的直接父目录已存在且符合[生成权限要求](config/README.md#示例与首次设置)；再运行 `sudo -u auction /opt/auction/bin/auction-admin-api setup --output-dir /var/lib/auction/new-config --control-group auction-worker-control`，其中 `new-config` 须尚不存在。在 stderr 中取得设置地址（默认 `127.0.0.1:8091`）与一次性令牌，经 SSH 隧道（`ssh -L 8091:127.0.0.1:8091 <主机>`）在浏览器完成表单。生成后把目录内容移入 `/etc/auction/` 并按上表调整属主与权限。
   - 手工：以 [`ops/config/examples/`](config/examples) 为模板编写，字段说明见[配置参考](config/README.md)。

4. **补齐关键配置**：
   - `secrets.toml`：`[database]`、`admin_actor`/`admin_secret`、三组数据密钥（`openssl rand -base64 32`，每组独立）。
   - `worker.toml`：`[worker.control] enabled = true`；`allowed_peer_uids` 须包含运行 Admin 的账号 UID，并且**包含 0**（`auction-stack.service` 以 root 运行 `auction-cli service`，需要探测 Worker 就绪）。
   - Public 的 `[http_security]` 与 Admin runtime 的 `[security]`：`allowed_origins` 设为对外 HTTPS origin，`cookie_secure = true`；反向代理在本机时 `trusted_proxy_cidrs = ["127.0.0.1/32"]`。Admin 角色文件的 fallback 安全配置也须合法；配置了 runtime 时以 runtime 为生效源。
   - 每个 Public 实例使用唯一的 `realtime.consumer_key`。

5. **安装运行时目录与 unit**（在解压后的发布包根目录执行；模板在仓库中位于 [`ops/deploy/tmpfiles/`](deploy/tmpfiles) 与 [`ops/deploy/systemd/`](deploy/systemd)）：

   ```bash
   install -m 0644 tmpfiles/hv-auction.conf.example /etc/tmpfiles.d/hv-auction.conf
   systemd-tmpfiles --create /etc/tmpfiles.d/hv-auction.conf
   for unit in auction-worker auction-admin-api auction-public-api auction-stack; do
     install -m 0644 systemd/$unit.service.example /etc/systemd/system/$unit.service
   done
   systemctl daemon-reload
   systemctl enable auction-stack.service     # 不要单独 enable 三个角色 unit
   ```

6. **检查配置**：`auction-cli --config-dir /etc/auction config check --strict`。

7. **迁移并启动**：`systemctl start auction-stack.service`（等价于 `auction-cli service start`：预检 → 迁移 → 按序启动并等待就绪）。用 `auction-cli service status` 确认三个角色均 ready。

8. **配置反向代理**（见下文），登录 Admin 后台。

9. **录入外部凭据**：在 Admin “Cookie 池”或用 `auction-cli credential import` 录入 MoogleMail 与论坛 Cookie，然后在 `worker.toml` 启用 `[worker.mooglemail]`、`[worker.forum]` 等功能，执行 `auction-cli service restart`。

### 合并部署

1. 安装 `auction-server` 与 `auction-cli`，建立 `auction` 账号，预先建立专用的**空**数据库与应用角色。
2. 把发布包中的 `systemd/auction-server.service.example`（仓库中为 [`ops/deploy/systemd/auction-server.service.example`](deploy/systemd/auction-server.service.example)）安装为 `/etc/systemd/system/auction-server.service`（配置目录 `/var/lib/auction/config`，运行目录 `/var/lib/auction/.config.server`），`systemctl enable --now auction-server`。
3. 配置目录不存在时进入首次设置：在 `journalctl -u auction-server` 中取得令牌，经 SSH 隧道访问 `http://127.0.0.1:8091/setup`，填写表单并确认“初始化并启动”。服务保存配置、把空库初始化到当前 schema，然后依次启动三个角色。
   - 无法使用 SSH 隧道时，可临时用发布包中的 `caddy/setup.Caddyfile.example`（仓库中为 [`ops/deploy/caddy/setup.Caddyfile.example`](deploy/caddy/setup.Caddyfile.example)）反代，并给 `auction-server` 加 `--allow-remote-setup --setup-origin https://<域名> --setup-proxy-ip 127.0.0.1`；完成后立即删除临时站点与参数。
4. 补齐外部凭据与功能开关后 `systemctl restart auction-server`。

初始化失败或结果未知时服务不会自动重试（unit 为 `Restart=no`）：保留配置与数据库现场，核对后用 `auction-cli db` 系列命令恢复。

## systemd 服务与顺序控制

| unit | 说明 |
| --- | --- |
| `auction-worker.service` | `Restart=on-failure`；`RuntimeDirectory=hv-auction/worker` 管理 control socket 目录；可写 `/var/lib/auction` |
| `auction-admin-api.service` | `Requires=`/`After=` Worker；附加组 `auction-worker-control` 以访问 control socket；可写 `/etc/auction/runtime` |
| `auction-public-api.service` | `Requires=`/`After=` Worker 与 Admin |
| `auction-stack.service` | oneshot、`RemainAfterExit`，以 root 执行 `auction-cli service start/stop`；开机按序拉起 |
| `auction-server.service` | 合并部署专用；`KillMode=mixed`、`TimeoutStopSec=225s`、`Restart=no` |

Public/Admin unit 保持 `TimeoutStopSec=45s`，Worker 为 `120s`，stack 为 `330s`；保留原有 systemd 加固选项。外层上限按“普通 + 关键段 + tracing 上限 + 余量”计算，三角色普通预算默认 10 秒，Worker 关键段默认 60 秒，tracing 上限 30 秒。supervisor 从 RolePlan 求和，默认 195 秒，server unit 为 225 秒。提高应用预算须同步提高相应 unit 上限，详见[排空与停止预算](config/README.md#排空与停止预算)。

Worker 停止时立即取消尚未登记交接的准备期工作并保留持久 claim。delivery、PM 和 compose discard 在持久围栏调用前登记，围栏调用开始即受保护；关闭先赢时不发起围栏，登记先赢时围栏往返也计入活动、阻止排空结束。登记本身不授权外写，只有围栏确认提交才继续 POST；确认失败或提交未知仍不执行外写。论坛在已有持久 claim 下的首个实际 POST 前登记，同一活动的后续必要 POST 不因关闭入口被截断。已登记交接与关键段的 owner 继续执行原续租、凭据校验和结果落库，共享一次 60 秒截止，之后普通收口最多 10 秒；仅在既有结果端口确认提交后 complete，其余由 drop 记未收口。60 秒只保证“剩余工作可在预算内完成时落库后退出，否则保持既有未知边界”；不延长请求、lease 或证据寿命。超时和二次信号非零退出，关闭路径不补造失败或人工态；二次信号跳过可选 tracing 刷新。旧路径可通过角色配置 `critical_write_timeout_seconds = 0` 回退，切换需重启。

W7.2 的在途发送门禁尚未实现，当前部署不受其保护。现有 `service stop/restart`、直接 systemctl 停止、主机重启等入口仍可能耗尽关键段预算，届时必须沿既有人工对账或只读证明流程处理，绝不自动重发。

**为什么用 `auction-cli service` 编排**：systemd 的 `After=` 只保证“进程已启动”，不保证“已就绪”。`auction-cli service start` 在每个角色启动后等待其 readiness（Worker 为 control socket 的 `/v1/health/ready`，Admin/Public 为 `/health/ready`，且期间 `MainPID`/`InvocationID` 不变）才启动下一个；单角色 readiness 的完整超时预算覆盖 unit 状态查询、探测、身份复核与轮询等待，`systemctl start` 自身另有 90 秒上限。任何一步失败都会反序停止本次启动的 unit。它还在启动前执行严格配置检查与数据库迁移。详见 [auction-cli](../apps/auction-cli/README.md#分体部署编排)。

常用操作：

```bash
auction-cli --config-dir /etc/auction service status     # 三个 unit 的状态与 readiness（未全部就绪时退出码 4）
auction-cli --config-dir /etc/auction service restart    # 预检新配置 → 时段门禁 → 反序停止 → 迁移 → 顺序启动
auction-cli --config-dir /etc/auction service stop       # 时段门禁 → 按 Public → Admin → Worker 停止
```

注意：

- 三个角色 unit 的 `Requires=`/`After=` 不提供应用 readiness 编排；单独重启 Worker 可能影响依赖角色的运行状态。需要整体重启时使用 `auction-cli service restart`。
- Worker 在凭据所有权失效、control 服务停止等运行时故障时以非零状态退出，由 `Restart=on-failure` 拉起。
- 迁移、建库、重建与恢复要求三个 unit 全部停止（或使用合并部署的 owner 锁），见[停机门禁](../apps/auction-cli/README.md#停机门禁)。

### 停机时段保护

`service stop/restart` 在真正停机前只读检查数据库：任一场次处于 Final，或任一在拍 lot 的有效截止不晚于未来保护窗口末端（含已过截止但仍为 live），就拒绝并以 4 退出，输出场次/lot ID 与截止时间。慢速代理按其 lot 的同一截止条件计入，调度到期时间本身不单独阻挡；Live 期间其他时段允许重启，代理恢复后顺延。数据库不可达、配置不可读或查询失败同样默认拒绝；`start` 不受影响。

保护窗口通过 `--protection-window-minutes` 设置（默认 60，范围 1–10080 分钟）。运维必须根据[迁移演练回执](deploy/MIGRATION-DRILL.md#停机时长与窗口)设为不少于实测停机时长的 3 倍；CLI 只校验范围。窗口只对本次命令生效，systemd 包装命令若未传参则仍用 60。紧急例外用 `service stop/restart --force --reason '具体原因'`，两者缺一即参数错误（退出码 2）；原因不可为空或包含秘密、玩家信息。已过截止却长期卡在 live 的 lot，须先人工排查生命周期与收尾故障，再记录原因强制停机。停止前的 stderr/journal 与成功/拒绝 JSON 记录 force、原因、证据与判定，后续失败仍可查看停止前声明。字段与区间边界见 [CLI 契约](../apps/auction-cli/README.md#停机时段门禁)。

**边界（W9.6）**：只读快照不冻结拍卖状态。直接 `systemctl stop/restart` 角色 unit、合并部署的 `auction-server` 停机、系统自动更新重启、关机与断电均可能绕过 CLI；即使 stack 的 ExecStop 被拒绝，也不能把它当成主机关机屏障。系统自动更新/维护重启、主机层排期与防绕过由 W9.6 约束，本批次没有实现主机拦截。W9.6 落地前由运维关闭或协调无人值守自动重启，按同一保护窗口安排维护，避免用直接 systemctl 绕开拒绝。

## 升级

分体部署：

1. 阅读版本说明；如有配置变更，先在新配置上执行 `auction-cli config check --strict`。含迁移的版本在生产停机前用最新备份完成[迁移演练](deploy/MIGRATION-DRILL.md)，审核回执并据实设置保护窗口；M1 手工核对，W7.2 落地后接入自动门禁。
2. 停服并建立确定的恢复点：`auction-cli service stop`，然后 `auction-cli backup create --database auction --output /var/backups/auction/auction-<日期>.dump`。
3. 保留旧程序以便回退，安装新程序到 `/opt/auction/bin/`。
4. `auction-cli --config-dir /etc/auction service start`：严格预检 → 迁移 → 按序启动并等待就绪。
5. `service status` 确认就绪，检查 Admin 仪表盘。

不需要确定恢复点的小版本（无迁移）也可以直接 `auction-cli service restart`：新配置预检通过后才停止旧服务，然后迁移并按序启动。

合并部署：`systemctl stop auction-server` → 备份 → 安装新程序 → 以服务账号执行 `sudo -u auction auction-cli --config-dir /var/lib/auction/config db migrate` → `systemctl start auction-server`。

迁移 `000069` 后新建场次使用统一加价与代理上限出价规则；升级前已经创建的场次（包括草稿与预览）继续原竞价策略，不自动切换。必须停止全部旧角色、备份、迁移后同版启动；不能让旧 Public/Worker 忽略新快照版本继续写入。场次快照约束见[数据库契约](../docs/contracts/database.md#拍卖与竞价)。

迁移 `000071` 把两个世界的补偿池余额相加，合并为一个以 main Credits 计的全局池，并冻结旧的按世界池表；旧程序写旧表会失败关闭，因此同样必须停止全部旧角色后迁移。合并规则见[结算与履约](../docs/guide/settlement.md#补偿池)。

**回退**：迁移只向前。W7.8a 已随一版部署后，目标旧程序已含兼容逻辑且本次跨越的迁移全部登记 compatible，才可保留当前数据库换回程序；换回前必须显式设置 `[readiness] migration_compatibility_mode = "compatible"`，保留能识别当前数据库的新版 CLI。默认 shadow 不放宽，含 breaking、缺登记或无法确认仍须恢复升级前备份并核对外部历史。完整区间判定、CLI 限制与操作步骤见[程序回退](deploy/RECOVERY.md#程序回退)。

## 反向代理

[`ops/deploy/caddy/Caddyfile`](deploy/caddy/Caddyfile) 是 Public 与 Admin 两个站点的模板：

- 自动 HTTPS；WebSocket（`/api/v1/ws`）由 `reverse_proxy` 直接透传。
- 对公网屏蔽 `/metrics`（返回 404）；只压缩 `/assets/*` 等静态资源。
- 上游默认 `127.0.0.1:8090`（Public）与 `127.0.0.1:8091`（Admin），可改为 Unix socket（同步 listener 的 `mode = "unix"`、`trusted_unix_proxy = true` 与 socket 组权限）。
- 服务端只采信来自 `trusted_proxy_cidrs`（或受信 Unix 上游）的 `X-Forwarded-For`，速率限制按真实客户端地址计数。
- 强烈建议 Admin 站点只对内网或经额外认证开放。

## 监控与日志

### 健康检查

| 目标 | 检查 |
| --- | --- |
| Public / Admin | `GET /health/live`、`GET /health/ready`（数据库按迁移模式校验、审计模式门禁通过；Public 另要求 realtime relay 新鲜；默认一秒进程缓存） |
| Worker | control socket 的 `GET /v1/health/ready`，例如 `curl --unix-socket /run/hv-auction/worker/control.sock http://localhost/v1/health/ready`（调用者须在 peer allowlist 中） |
| 全部 | `auction-cli service status`（分体部署） |

三个角色每 10 秒向数据库写入心跳（TTL 30 秒）；Admin 仪表盘汇总各进程状态，过期心跳显示为缺失。

HTTP 缓存的转换时限与异常行为见[路由契约](../docs/contracts/routes.md#健康与指标publicadmin)。心跳不复用 HTTP 缓存。Worker control 仍只检查运行/排空状态，审计失败影响其数据库心跳，不扩展 control 契约。`auction-server` 启动时等待探针，全部启动后只监管子进程退出；systemd 模板没有 HTTP 主动检查，Caddy 模板也未配置主动健康探针，readiness 503 本身不会触发 systemd 重启。

### 审计就绪影子观察

默认 `[readiness] audit_failure_mode = "shadow"`、`cache_enabled = true`。该模式保持旧就绪判定及心跳全部失败累计，同时在数据库检查成功后比对审计门禁。`auction_readiness_shadow_mismatch_total{class}` 按发生差异的底层检查次数累计，不是失败命令数或 HTTP 等待者数；只含瞬时失败且尚无阻断类失败时，旧门禁拒绝、新门禁放行，这是预期差异。完整分类字典见 [auction-storage](../crates/auction-storage/README.md#审计失败与就绪)。

1. 用户先确保指标已有可采集和查看的路径。Public/Worker 的指标取 common，Admin 若有 runtime 则须同时检查其 metrics 开关；Worker 当前从 control socket 抓取。本项不安装采集服务、不启用外部心跳或运维告警通知。
2. 保持 shadow 观察。日志 target 为 `auction::storage::audit_health`，只输出 `class`、`sqlstate_class`、固定 `kind`、`blocks_classified_readiness`；最后一项明确指 classified 门禁，shadow 的实际门禁仍会被瞬时累计阻断。需要开启时在既有 `log_filter` 追加 `,auction::storage::audit_health=warn`，关闭追加 `,auction::storage::audit_health=off`；修改过滤器也需重启。
3. 按下列 PromQL 核对分类、影子差异、阻断状态和 HTTP 检查收敛。差异只允许出现在 transient，permanent/unknown 无差异；观察必须有对应分支样本，无样本不算通过。不得为取样擅自连接生产数据库或制造故障。
4. 用户审阅证据并决定是否将 common 改为 `audit_failure_mode = "classified"`，按适用的停机门禁受控重启相关角色。不会自动切换；分类必须先于 W0.6 超时启用。classified 下心跳和 Admin 计数只含 permanent + unknown，瞬时完整计数继续在指标与日志中保留。
5. 回退时设置 `audit_failure_mode = "legacy"`、`cache_enabled = false`，受控重启恢复任一失败即未就绪与逐次探测。不可重试/未知失败修复后也需受控重启，不提供清零接口、不写试探审计。单独禁用 metrics 或过滤日志不影响门禁。

```promql
# 当前进程的分类失败累计；包含随后回滚并重试成功的尝试，首次出现的 label 也可直接查看。
sum by (instance, class, sqlstate_class, kind) (auction_audit_write_failures_total)
# 影子期预期差异；后两类必须为零，缺少实际失败样本须另行说明。
sum by (instance, class) (increase(auction_readiness_shadow_mismatch_total[1h]))
# 当前模式是否被审计累计阻断。
auction_audit_readiness_blocked
# 比较 HTTP 请求与真实底层检查，观察命中、刷新和并发合并。
sum by (instance, outcome) (rate(auction_readiness_cache_requests_total[5m]))
sum by (instance, result) (rate(auction_readiness_checks_total[5m]))
histogram_quantile(0.95, sum by (instance, le) (rate(auction_readiness_check_duration_seconds_bucket[5m])))
```

已有查询为审计影子比对复用，不双查数据库。新指标不改变事务重试、业务写入与结果未知的处置；在决策 32 的白名单基础上，依决策 71 加入 `53300`（连接数耗尽）、`57P01`（服务器关闭）、`57P02`（崩溃关闭）、`57P03`（尚未就绪），固定 kind 分别为 `connection_capacity`、`server_shutdown`、`server_crash`、`server_starting`。`57P05`、`25P03/25P04` 仍按未知粘滞阻断。依决策 72，默认 shadow 上线，只允许预期 transient 差异，经用户审阅后决定是否切换 classified。

### 指标

Public/Worker 在 `common.toml` 设置 `[metrics] enabled = true`；Admin 配置了 runtime 文件时，enabled 以 runtime 的 `[metrics]` 为准。Public/Admin 在各自 listener 上提供 `/metrics`（listener 必须是 loopback TCP 或 Unix socket），Worker 在启用的 control socket 上提供 `/metrics`。指标族见 [auction-observability](../crates/auction-observability/README.md#指标)。建议关注：

- 按路由模板的 HTTP 请求数、状态码和延迟，已升级 WebSocket 当前/累计数，login/mutation/realtime 频率拒绝；
- Worker 循环迭代与调度延迟、批次饱和、重试次数；
- 数据库连接池等待与空池次数；
- tracing 导出失败或降级；
- 业务事件计数。

Public/Admin 默认开启部署层 `common.toml [metrics] reject_forwarded_requests = true`：带转发头的任意方法在应用层返回空 404，优先于 request-id 400，不带 `Allow`，与 Caddy 的公网 404 屏蔽并存。精确头集合与直连方法见[路由契约](../docs/contracts/routes.md#健康与指标publicadmin)。CDN 专用来源头待 W0.12 核实后补充；代理剥除所有识别头时，仅靠应用层头检查无法识别转发，因此必须保留 listener 限制和 Caddy 屏蔽。

采集器应直接请求本机 loopback `/metrics`；Unix listener 可通过支持 Unix socket 的采集客户端直接访问实际 `listener.socket_path`，不要通过 HTTP 反向代理或附加转发头。以下是 TCP 直连和 SSH 隧道示例，Admin 可改用 8091；`auction-host` 替换为部署主机的 SSH 地址：

```bash
# 在部署主机直接访问 Public 的 loopback listener。
curl --fail http://127.0.0.1:8090/metrics
# 从本机建立隧道；另一终端访问映射端口，不经过 HTTP 代理。
ssh -N -L 18090:127.0.0.1:8090 auction-host
curl --fail http://127.0.0.1:18090/metrics
```

回退时仅将 common 中的 `metrics.reject_forwarded_requests` 设为 false，并按时段门禁重启 Public/Admin，恢复旧访问行为；runtime 对 enabled 的覆盖不会覆盖此开关。该字段不在 Admin 参数文档中，修改需重启；保留 Caddy 屏蔽和 loopback/Unix 限制。关闭 `metrics.enabled` 可停用指标 collector，所有 HTTP 记录与连接观测守卫成为 no-op。Worker control 继续按 peer UID/GID 授权；新增 loopback 采集出口与指标存储由 W0.10 处理。

### 运维告警

启用 `[worker.operations_alert]`（需要 MoogleMail 已启用且收件人在同一世界）后，Worker 定期扫描并通过 MoogleMail 发送告警邮件：投递积压（数量 / 最老年龄）、需要人工对账的投递、履约积压、付款逾期以及进程健康。同一信号有冷却时间（默认 30 分钟），通知按窗口合并。

### Admin 仪表盘

仪表盘集中展示：进程与 Worker 状态、各邮箱轮询、realtime 滞后与失败、凭据健康、投递/履约/恢复积压与告警状态。各分区独立降级。投递失败只统计仍待处置的失败：关注提醒的预检退役（从未开始发送）、Admin 对账判失败（以命令账本中产出当前版本的失败命令为证）、装备损失退役的原投递与已核验的 legacy 自寄修复不计入；履约失败不计入装备损失处置后转为操作者线下承担的买方退款。资金为“仅记账”时两个世界的恢复数据源都显示不适用，不读取外部余额。

### 日志

- 所有进程向 stdout 输出 JSON 日志（无 ANSI），由 journald 收集：`journalctl -u auction-worker -o cat | jq`。
- 过滤器由 `common.toml` 的 `log_filter` 控制（默认 `info,<服务名>=debug`），使用 `EnvFilter` 语法。
- 日志只包含安全分类与标识（命令 ID、拍卖/lot/义务 ID、错误码），不含连接串、Cookie、密钥、登录 key 或邮件正文。
- 可选 OTLP/HTTP tracing：在角色文件 `[tracing]` 启用，导出头只能写在敏感文件的 `*_otlp_headers` 中（进程环境中的 `OTEL_EXPORTER_OTLP_*HEADERS` 会导致拒绝启动）。

### D1 诊断

COD 实收资金化诊断使用独立 target `auction::settlement::funding_diagnostics`。默认过滤器 `info,<服务名>=debug` 已包含其 WARN，因此默认开启；显式自定义过滤器若屏蔽该 target，可在 `common.toml` 的现有 `log_filter` 末尾追加 `,auction::settlement::funding_diagnostics=warn`。关闭时追加 `,auction::settlement::funding_diagnostics=off`。例如，以下是开启配置，保留现场其他过滤指令后再追加目标指令：

```toml
log_filter = "info,auction::settlement::funding_diagnostics=warn"
```

关闭配置示例：

```toml
log_filter = "info,auction::settlement::funding_diagnostics=off"
```

过滤器在初始化时读取，修改后须按原停机门禁重启。关闭 target 同时停用其日志、诊断 span 和授权无行后的额外诊断查询；原 W0.1 Worker 警告与业务流程保持。也可换回旧程序，不涉及迁移或数据回退。

先按 target 收窄，再只输出允许的字段；JSON 日志的义务/投递/命令标识可能位于父 `spans` 或当前 `span`：

```bash
journalctl -u auction-worker -o cat |
  jq -c '
    select(.target == "auction::settlement::funding_diagnostics") |
    ([.spans[]?, .span?] | add // {}) as $context |
    {
      obligation_id: ($context.obligation_id // .fields.obligation_id),
      delivery_id: ($context.delivery_id // .fields.delivery_id),
      command_id: ($context.command_id // .fields.command_id),
      world: ($context.world // .fields.world),
      stage: .fields.stage,
      safe_kind: .fields.safe_kind,
      cause_kind: .fields.cause_kind,
      defer_reason: .fields.defer_reason,
      outcome: .fields.outcome
    }'
```

对上述投影再用 `jq 'select(.safe_kind == "funding_receipt_send_before_boundary")'` 或 `jq 'select(.stage == "receipt_relation")'` 过滤；关联同一 obligation/delivery/command 的前置拒绝、回滚与 defer 失败。位置分类与 `cause_kind` 见 [Application 错误说明](../crates/auction-application/README.md#资金与内部证据错误)，收据/价格码与辅助 SQL 约束见 [Storage 唯一拒绝字典](../crates/auction-storage/README.md#cod-实收资金化拒绝诊断)，不要复制字典到其他文档。

辅助查询只在原授权 SELECT 无行且 target 开启时执行：同事务、只读、只返回布尔谓词。剩余证据预算小于一秒时跳过，否则先设置事务局部 `statement_timeout='250ms'`，完整等待服务端查询结果，不在客户端取消 future。FALSE/NULL 都不通过；预算不足、SET/查询失败或超时归回原泛化拒绝，始终返回原 `Unproven` 并进入原回滚路径，详细预算与中止事务的回滚语义见 [Storage 说明](../crates/auction-storage/README.md#授权查询无行的只读诊断)。两种原 defer reason、白名单、读取契约与人工证明都不变。

D1 仍待线上分类证据确认。部署后由用户执行只读核对，数据库查询放在只读事务内，仅选必要标识、状态、时间和布尔比较结果：

1. 确认实际 Worker 制品版本、迁移版本，以及同一义务的原 unpersisted 警告与本 target 的日志链。
2. 核对 payment 的 execution/business state、last_error_code、retry_count、next_attempt_at、版本与租约时间，以及 seller blocked/refund contingent 状态。
3. 若出现发送边界类别，核对原 sending_at 与账户模式对应的最新 record_opening/reconciliation 时间；只输出来源存在与严格先后比较，不导出 UID 或金额。
4. 若出现发送证明类别，核对 operator/frozen 强谓词及 confirmation、attempt、binding、审计、原 Admin 失败命令的关联布尔值，不导出正文、payload、metadata、快照或摘要。
5. 核对稳定内部/资金命令、funding lot、receipt 的存在性，区分重放、业务拒绝、已确认回滚、未确认结果和 defer 持久化失败；预算类结合日志时序核对证据与 claim 窗口。

正常资金拒绝会先回滚再尝试保存 Deferred，不能仅凭拒绝码认定它解释了 unpersisted；还需检查外层超时、早期错误或 defer 失败。确认根因后另行批准修复；不据诊断修改人工 Sent、更正原失败命令或自动重发。

## 日常运维

| 任务 | 做法 |
| --- | --- |
| 更换外部 Cookie | Admin “Cookie 池”录入新凭据并停用旧凭据（每种 kind 同时只能一个 active），或 `auction-cli credential import`；然后重启 Worker（分体部署用 `service restart`） |
| 暂停/恢复收件 | Admin 仪表盘或“收件审核”页；也可立即触发一次轮询。恢复轮询不绕过场次冻结的赛季收件门禁，见[赛季排期](../docs/business-rules.md#赛季早鸟期auctionscheduleearly_bird) |
| 结果未知的投递 | Admin “投递队列”按动作与证据人工对账；主动 Send 禁止仅凭外部引用人工确认成功，须由冻结的精确 Sent 证明恢复。确认失败或授权重试前核对远端实际结果，未找到 Sent 不证明未发送 |
| 恢复确认 | Admin “结算报表”中确认 `reclaimed` / `cancelled` / `equipment_recovered`；付款截止与跨赛季重列限制见[恢复规则](../docs/guide/settlement.md#4-恢复付款超时与拒收) |
| 论坛写入未知 | `auction-cli forum recovery` 查看，必要时 `forum authorize-retry` |
| 论坛私信对账 | `auction-cli pm list` / `pm reconcile` |
| 规则发布 | Admin “规则内容”页，或 `auction-cli rules import` |
| 修改运行参数 | Admin “参数配置”页（校验后原子写入 runtime 文件），随后重启相关角色生效 |
| Bot 资金账户 | `auction-cli funds preview` 可在线只读执行；开户或现金对账写入前须先停止 Worker，见[结算与履约](../docs/guide/settlement.md#bot-内部履约与资金账户) |
| 导出邮件诊断证据 | `auction-cli mail export-evidence <evidence-id> --output <文件>` |
| 数据保留盘点 | `auction-cli db inventory`（只统计，不授权删除） |

## 备份与恢复

例行备份使用 `auction-cli backup scheduled` 配合独立 systemd timer，包含磁盘门禁、本地保留和不可变回执；启用、回退与故障处理见[定时备份](deploy/RECOVERY.md#定时备份)。外部打卡暂缓启用，异机加密上传另行交付；当前仍需把 archive 与 manifest 一起复制到异地。升级前确定恢复点继续使用手动 `backup create/verify`。恢复、重建与演练见[备份与恢复手册](deploy/RECOVERY.md)。
含迁移版本的升级演练使用独立的[迁移演练手册](deploy/MIGRATION-DRILL.md)与[回执模板](deploy/migration-drill-receipt.template.json)，恢复过程仍复用上述手册。

## 故障排查

| 现象 | 排查 |
| --- | --- |
| 进程不就绪，日志提示 schema 版本不符 | 停服后执行 `auction-cli db migrate`（分体部署用 `service restart`） |
| `service start` 卡在 Worker readiness | 确认 `[worker.control]` 已启用、`allowed_peer_uids` 含 0、`/run/hv-auction/worker` 存在且权限正确 |
| Admin 启动报 runtime 文件与启动配置不一致 | runtime 文件已被更新但进程尚未用新配置重启：用 `auction-cli service restart` 整体重启 |
| Worker 启动失败：凭据所有权已被占用 | 另一个 Worker（或正在执行的 `auction-cli funds`）持有邮件/论坛 Cookie 所有权；确认只运行一个 Worker |
| Public 启动失败：realtime consumer 已被占用 | 两个 Public 实例使用了相同的 `realtime.consumer_key` |
| 浏览器写请求返回 `origin_forbidden` | `allowed_origins` 与浏览器访问的 origin 不完全一致（协议、主机、端口） |
| 所有客户端被同一地址限流 | 反向代理地址未加入 `trusted_proxy_cidrs`，服务端把代理当作客户端 |
| 页面返回 503 | 程序不是 `web-embed` 构建，或内嵌资源缺失 |
| 投递长时间停在 `manual_reconcile` | 结果未知，需在“投递队列”人工对账；不要直接改数据库。对账返回 `delivery_sent_sighted` 时说明 Sent 已有该结算义务的邮件，改用 `auction-cli settlement confirm-sent` |

服务 owner 回收后仍监听终止信号；tracing 刷新期间再收到信号时，装配根直接非零退出，避免不可取消的刷新线程延迟强退。

### SQL 失败诊断

先按既有命令、拍卖、lot 或投递标识关联失败日志。带安全上下文的错误在原中文原因后附加方括号字段：`sqlx_kind` 区分数据库错误与连接池、传输、解码等失败；`sqlstate_class` 为两位类别，`sqlstate` 为五位代码；`constraint`、`table` 可定位迁移中的约束、索引或表。字段规则的唯一说明见 [Storage SQL 错误安全上下文](../crates/auction-storage/README.md#sql-错误安全上下文)。

`none` 表示驱动未提供该字段，`invalid` 表示诊断字段格式异常；部分触发器错误没有约束名或表名，不能据此认定没有约束拒绝。结合制品/迁移版本、原业务上下文和持久状态排查，不收集驱动 message/detail/hint、SQL、绑定值、连接串或凭据。仅记录 `safe_kind` 的日志仍只有分类，并非每个失败入口都输出增强文本。

SQLSTATE 和错误文本用于定位原因，不能据此自动重试、重发邮件或认定提交已回滚；继续遵循原事务 owner 的类型化判定与未知结果对账流程。本项不新增配置，回退为换回旧制品；日志过滤器不能撤销已经写入日志或失败安全详情的增强文本。
