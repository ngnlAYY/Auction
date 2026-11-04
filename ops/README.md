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

### M1 候选上线顺序（备份与指标前置）

线上旧 CLI `f99a9d4e` 早于 `backup scheduled` 与三角色 loopback metrics 出口，不能在旧程序上启用新 timer 或要求出口验收。正确顺序：

1. **部署前准备，不改变旧角色**：R2 bucket/生命周期/bucket lock/令牌；`age` 密钥离线托管与主机 `age`；Prometheus、node_exporter、postgres_exporter 的软件、unit 与 `auction_metrics` 只读账号。监控服务可以先启动，此时 scrape 目标不可用是预期的。
2. **安装候选**：按“升级”流程安装 M1 候选（含迁移 075–077），同步安装 Worker 120s、server 225s 的 systemd unit；`service start` 完成迁移并等待三角色就绪。
3. **启用新版能力**：在 common 填写三个 loopback metrics 地址并受控重启角色；安装并启用 `auction-backup.timer`，先执行一次手动 `backup create`/`backup verify`。
4. **异机与恢复演练**：配置 `[backup.offsite]`，观察首次 daily/monthly 回执，再按[备份与恢复](deploy/RECOVERY.md#从-r2-取回并演练恢复)完成一次端到端恢复演练。
5. **观察后切换**：维护 p95、D5 冲突、shadow 差异与备份连续成功后，才切换 `readiness_audit_failure_mode=classified`、`timeouts_enabled` 与 `sent_baseline_mode=watermark`。

在旧程序上执行第 3–5 步会因缺少子命令或出口失败；这些步骤不属于部署前准备。

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
4. 用户审阅证据后，可按[运行时开关](#运行时开关)将 `readiness_audit_failure_mode` 设置为 `classified`，核对三角色应用后生效；修改 common 基线则仍需受控重启。不会自动切换；分类必须先于 W0.6 超时启用。classified 下心跳和 Admin 计数只含 permanent + unknown，瞬时完整计数继续在指标与日志中保留。
5. 回退时可在线设置 `readiness_audit_failure_mode=legacy`、`readiness_cache_enabled=false`，恢复任一失败即未就绪与逐次探测；clear 只回落 TOML 基线。不可重试/未知失败修复后也需受控重启，不提供清零接口、不写试探审计。单独禁用 metrics 或过滤日志不影响门禁。

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

### Sent 发送基线影子观察

W1.4 本切片仅处理发送前基线（D3）。默认 `[worker.mooglemail] sent_baseline_mode = "shadow"`，旧完整排除集唯一裁决，水位分支复用同一读取轮次的页面，比较新增 MID 集合与收件人/标题列表投影候选集合，不执行额外 POST。临时边界随 prepared 句柄一次消费，不新增持久水位表；常规轮 Sent/Read Mail 水位与每日校验留待后续交付。D3 提前切片不以 W6.3 为前置（决策 74），不代表批准测试、浏览器操作或外部访问。

1. 用户先确认 Worker 指标已有采集与查看路径，保持 shadow。按[指标字典](../crates/auction-observability/README.md#sent-发送基线局部影子指标)核对四族，分别查看 world 和 branch 样本；零值或没有样本不能解释为通过。
2. 另行授权浏览器只读核实：Sent 的 MID 格式、创建顺序是否单调且不复用，首页最新分钟组最大 MID 是否为全箱最大值，跨分钟是否有反例；确认实际每页行数、同分钟并列行顺序与跨页表现。分别查看 main/isekai，并记录 HV 清理、历史回填和换季代次现象。只读 Sent 列表，不打开 Bot Inbox 未读邮件、登录 key 邮件，不为采样发信或删除邮件；有限样本不视为永久协议保证，运行时守卫和失败关闭仍保留。
3. 安排 7–14 天观察，并至少覆盖一个包含 Final 与结算的完整场次周期；周期更长时顺延。各判定分支有实际样本且非预期差异为零，超时、取消或窗口不一致的样本单列。缺样本分支须补充获准的材料或由用户明确决定例外，不能只凭日历时间切换，也不擅自制造故障。
4. 正常可比较样本的 MID 与候选集合差异均必须为零。旧完整扫描触顶而水位可取得边界，是预期可用性改善；shadow 仍会阻断发送，因此该次没有发送后对照，必须计不可比较。`cache_boundary_delta` 需调查捕获窗口变化，不能归为 D3 的预期候选差异。`invalid_mid/order_violation/anchor_changed/scope_changed/response_hint_conflict` 等前提异常未解释并排除时不得切换，不能只看 mismatch 为零。
5. MID 语义核实成立、观察与分支覆盖通过后，由用户决定将 TOML 改为 `watermark` 并受控重启。该字段暂不接入 W7.7 在线开关，重启受 W7.5 部署时段门禁约束。默认 shadow 本身不解除 D3，必须在旧路径页数或预算触顶之前完成切换。
6. 回退时设置 `full` 并受控重启；完整扫描仍有 100 页上限，触顶后的旧路径只能失败关闭，不保证恢复发送。水位路径写前异常回退完整扫描；写后锚点消失则完整枚举但使用原边界；写后前提失效且没有发送前完整集合则保持未知。所有回退共用原预算、限速与资源限制，找不到 Sent 不等于未发送，绝不自动重发。换回旧二进制前移除其不认识的新配置字段。

```promql
sum by (instance, world, branch) (increase(auction_mooglemail_sent_baseline_shadow_evaluations_total[24h]))
sum by (instance, world, class) (increase(auction_mooglemail_sent_baseline_shadow_mismatch_total[24h]))
sum by (instance, world, reason) (increase(auction_mooglemail_sent_baseline_shadow_uncomparable_total[24h]))
sum by (instance, world, phase, reason) (increase(auction_mooglemail_sent_baseline_fallback_total[24h]))
```

### 指标

Public/Worker 的 `metrics.enabled` 来自 common；Admin 使用 runtime 文件时，
只有 Admin 的 enabled 由 runtime 覆盖，附加地址与拒绝策略仍来自 common。
业务 listener/control 的指标路径保留；Unix socket 部署可另开本机 TCP 出口。
完整字段与校验见[配置参考](config/README.md#commontoml)，
指标族见[observability README](../crates/auction-observability/README.md#指标)。

附加出口只挂载 `/metrics`，使用同一注册表，不提供健康、控制或业务接口。
三角色新出口与 Public/Admin 原指标入口共享方法、query、转发头拒绝和响应实现；
默认带识别转发头的任意方法返回空 404，拒绝优先于 request-id 400。
Worker control 仍按原 UID/GID allowlist 授权。
精确头集合及状态码见[路由契约](../docs/contracts/routes.md#健康与指标publicadmin)。
Caddy 的公网 `/metrics` 404 规则继续保留，采集不经过 Caddy 或 Cloudflare。

绑定或服务失败只降级采集，不改变 readiness，也不触发 Worker 业务退出守卫。
查看固定 ERROR 类别 `metrics_bind_failed`、`metrics_serve_failed`、
`metrics_serve_stopped`；任务随角色退出取消，scrape 不查询数据库或外部服务。

#### Prometheus 与 exporter（用户安装）

以下安装、授权、服务启动与核对均由用户执行，仓库静态门禁不代替这些步骤。
已记录主机内存约 1.9 GB、磁盘余量有限，先保留 15 天；
观察内存、CPU、磁盘及 TSDB 增速后才考虑调至 30 天。
若另加空间上限，达到上限可能早于保留期淘汰数据，须单独核对实际覆盖天数。

1. **固定版本。** 安装 Prometheus、node_exporter、postgres_exporter，
   记录来源和版本，核对各自帮助及 collector 名称。
   本仓库不预设 exporter 版本；监听参数及自定义查询接口按安装版本核对。
   确认 node 的磁盘、文件系统和内存指标，以及下表 PostgreSQL 覆盖项。
   模板中的二进制路径默认 `/usr/local/bin/`，按实际安装位置修改。
2. **账号授权。** 由用户建立无交互系统账号 `prometheus`、`node_exporter`、
   `auction_metrics`。PostgreSQL 账号由 DBA 执行以下 SQL 创建；
   示例数据库为 `auction`，须换成实际目标库，不使用应用 owner 或超级用户采集。

   ```sql
   CREATE ROLE auction_metrics LOGIN
     NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS
     CONNECTION LIMIT 2;
   GRANT CONNECT ON DATABASE auction TO auction_metrics;
   GRANT pg_read_all_stats TO auction_metrics;
   GRANT EXECUTE ON FUNCTION pg_catalog.pg_ls_waldir() TO auction_metrics;
   ALTER ROLE auction_metrics SET default_transaction_read_only = on;
   ALTER ROLE auction_metrics SET statement_timeout = '2s';
   ALTER ROLE auction_metrics SET application_name = 'auction-postgres-exporter';
   ```

   不授予应用表读写、对象所有权、`pg_read_server_files` 或整个 `pg_monitor`。
   默认只读参数不代替 ACL。`pg_read_all_stats` 可读取统计详情，查询只导出聚合，
   不输出 SQL、PID、用户名、客户端地址或 WAL 文件名。
   禁用需要额外权限的非必需 collector。
   优先使用本机 Unix socket 的 peer 认证，操作系统用户名与数据库角色同名。
   由用户检查 `pg_hba.conf` 的匹配顺序，目标库规则可使用：

   ```text
   local auction auction_metrics peer
   ```

   若采用密码认证，只放服务的私有凭据文件，不放仓库或命令行。
3. **配置与 unit。** 从仓库根复制下列模板，并核对二进制路径、用户及权限：

   ```bash
   install -d -m 0755 /etc/prometheus
   install -m 0644 ops/monitoring/prometheus.yml.example /etc/prometheus/prometheus.yml
   install -m 0644 ops/monitoring/auction-recording.rules.yml \
     /etc/prometheus/auction-recording.rules.yml
   install -m 0644 ops/monitoring/postgres-queries.yml.example \
     /etc/prometheus/postgres-queries.yml
   install -m 0644 ops/monitoring/prometheus.service.example \
     /etc/systemd/system/auction-prometheus.service
   install -m 0644 ops/monitoring/node-exporter.service.example \
     /etc/systemd/system/auction-node-exporter.service
   install -m 0644 ops/monitoring/postgres-exporter.service.example \
     /etc/systemd/system/auction-postgres-exporter.service
   ```

   为 `/etc/prometheus/postgres-exporter.env` 设置 root owner、
   `auction_metrics` group 与 0640 权限；按安装版本确认认证输入。
   支持 `DATA_SOURCE_NAME` 的版本可使用以下非敏感 peer 配置，
   其中库名与 socket 目录必须替换为实际值：

   ```text
   DATA_SOURCE_NAME='postgresql:///auction?host=/var/run/postgresql&user=auction_metrics'
   ```

   自定义查询模板仅补 WAL 字节数、长事务和锁等待聚合。
   按安装版本启用归档、表统计和 vacuum 进度 collector，
   将已核实的参数写入对应 unit；模板不猜测这些可选 flag。
   **复制文件不会自动启用查询**：必须按固定版本的配置接口接入，
   unit 注释保留了待核对项，不预填未确认的查询 flag。
   只启用内置 collector 缺失的项，每个指标保留一个来源；
   若版本不支持该扩展格式，先选择并固定支持所需覆盖项的版本。
   用安装版本的 `promtool check config` 核对 Prometheus 配置，
   再由用户执行 `systemctl daemon-reload`，启用三个监控 unit。
   这些 unit 与 Auction 角色之间没有 Requires 关系。
4. **启用角色出口。** 在 common 开启 metrics，按需填写三个可选地址：
   Public `127.0.0.1:19100`、Admin `127.0.0.1:19101`、Worker `127.0.0.1:19102`。
   配有 Admin runtime 时，另确认其中 `metrics.enabled = true`。
   按配置检查和部署时段门禁重启对应角色，不修改 Worker control allowlist。
5. **监听核对。** 用 `ss -ltnp` 检查三个角色出口、9090、9100、9187
   均只绑定 `127.0.0.1`，没有通配或 IPv6 监听；
   同时确认原业务 Unix socket/control socket 可用。
6. **采集核对。** 在部署主机直接请求三个角色及两个 exporter 的 `/metrics`，
   不附转发头、不经过 HTTP 代理；在 Prometheus 中检查以下五项均为 1：

   ```promql
   up{job=~"auction-public|auction-admin|auction-worker|node|postgres"}
   ```

   `up = 1` 只证明 scrape 成功；还须核对 exporter 的数据库采集错误指标与日志，
   以及下表各项实际存在且持续更新。缺 collector 不能算验收通过。
7. **资源观察。** 核对监控进程的内存、CPU、磁盘写入、抓取耗时及错误，
   特别关注文件系统剩余空间、可用内存、TSDB 增速和 PostgreSQL 连接占用。
   长事务/锁等待查询只返回计数和最老秒数，不能按会话扩张 label。
8. **SSH 隧道查看。** 在本机执行下列命令，`<host>` 换成部署主机：

   ```bash
   ssh -N -L 19090:127.0.0.1:9090 <host>
   ```

   浏览器打开 `http://127.0.0.1:19090`，不要为监控新增公网反向代理。

| 必须核对的 PostgreSQL 指标 | 来源与口径 |
| --- | --- |
| 归档 | 内置 collector 读取 `pg_stat_archiver` 的计数及最近成功/失败时间 |
| WAL 大小 | 内置 collector 或补充 `pg_ls_waldir()` 的 size 总和，单位字节 |
| autovacuum | 核对 `pg_stat_user_tables` 的 dead tuples、次数和最近时间 |
| vacuum 进度 | 核对 `pg_stat_progress_vacuum`，允许无在途任务时为空 |
| 长事务 | 内置或补充查询；未结束数量、事务内空闲数量及最老秒数 |
| 锁等待 | 内置或补充查询；未授予锁数量及 `waitstart` 最老等待秒数 |

归档尚未启用时，不因没有最近归档时间直接判定失败；
启用归档后的验收再核对成功/失败计数及时间是否推进。
查询使用专用监控角色，应用连接池与业务状态不依赖这些指标。
本批只交付采集路径与安装示例，不启用外部心跳、Webhook 或邮件告警。

回退：删除对应 `*_loopback_listen_address`，按部署窗口重启该角色，
原业务 listener/control 指标路径保留。关闭有效 `metrics.enabled` 可停用 collector。
若单独回退转发拒绝，在线开关不能放宽；修改 common 的
`reject_forwarded_requests = false` 并重启对应角色；保留 Caddy 公网屏蔽和监听限制。
监控服务由用户独立启停，不绑定 Auction readiness。

#### 外部账号客户端查询示例

W0.4/W0.5 的指标只在 Worker 装配真实记录器；关闭 metrics 时记录器为 no-op，
series 不出现，不会以零值替代缺测。启用或关闭都按配置与重启窗口执行。
进程重启后计数器归零，查询使用 `rate`/`increase`，不要跨重启直接比较原始值。
客户端、Worker、HTTP 与数据库层的失败计数不可相加当作“总失败”；
这些指标只覆盖外部账号客户端，不替代 W0.3、W0.6 或 W0.9。
采集与保存仍以 W0.10 的 loopback 出口和 Prometheus 为前提。

维护耗时 p95（沿用 W1.0 轮次直方图，不另建同义族）：

```promql
histogram_quantile(0.95,
  sum by (le) (
    rate(auction_worker_loop_iteration_duration_seconds_bucket{loop=~"mail-maintenance-.*"}[5m])
  ))
```

请求门等待 p95 与邮件预算耗尽速率：

```promql
histogram_quantile(0.95,
  sum by (le, world, operation) (
    rate(auction_mooglemail_request_gate_wait_seconds_bucket[5m])
  ))
sum by (world, phase, budget) (
  rate(auction_mooglemail_budget_exhaustions_total[5m])
)
sum by (world, phase) (
  rate(auction_forum_budget_exhaustions_total[5m])
)
```

论坛主题扫描的平均发现/处理页数与处理 p95：

```promql
sum by (world, outcome) (rate(auction_forum_topic_scan_pages_sum{measure="discovered"}[5m]))
  / sum by (world, outcome) (rate(auction_forum_topic_scan_pages_count{measure="discovered"}[5m]))
histogram_quantile(0.95,
  sum by (le, world, outcome) (
    rate(auction_forum_topic_scan_pages_bucket{measure="processed"}[5m])
  ))
```

回帖延迟覆盖与上界 p95；observed 只表示有有效窗口样本，不是最终接受延迟：

```promql
sum by (world, phase, outcome) (
  rate(auction_forum_reply_delay_samples_total[5m])
)
sum(rate(auction_forum_reply_delay_samples_total{outcome="observed"}[5m]))
  / sum(rate(auction_forum_reply_delay_samples_total[5m]))
histogram_quantile(0.95,
  sum by (le, world, phase, bound) (
    rate(auction_forum_reply_delay_estimate_seconds_bucket[5m])
  ))
```

`lower/midpoint/upper` 的分位差只是分位估计区间，不是逐样本宽度；
Final 分位目标、允许误差与最低覆盖率由 W0.13 定义，缺测不能解释为零延迟。

### SLI 基线与容量观察

先部署本批程序并按 W0.10 开启 Worker 的 metrics 采集；
已有数据库 076 提供运行时开关，本批不新增迁移或索引。
`common.toml [metrics] sli_snapshot_enabled` 默认 false，Admin runtime 不覆盖。
TOML 基线须启动加载；已有有效 metrics 后，可由用户在线设置开关：

```bash
auction-cli --config-dir /etc/auction switch list
auction-cli --config-dir /etc/auction switch set metrics_sli_snapshot_enabled true \
  --command-id <稳定UUID> --expected-revision <list中的版本> \
  --no-expiry --actor <操作者说明> --reason <非敏感原因>
```

数据库覆盖通常在下一次五秒刷新应用；汇总按六十秒唤醒，
因此还要等待下一轮成功快照。默认禁用不发 SLI SQL；metrics 未启用或无数据库时不建任务。
Admin 只读卡片展示三个数据库意图、两个 Admin 实际项，不能代替 Worker 应用确认。
更新后刷新旧 Admin 页面；Worker 实际开关可查看：

```promql
auction_runtime_switch_effective_value{role="worker",switch="metrics_sli_snapshot_enabled"}
auction_sli_snapshot_enabled
auction_sli_snapshot_available
auction_sli_snapshot_age_seconds
auction_sli_snapshot_refreshes_total
```

available=0、age=-1 表示尚未成功，不能当作业务延迟为零。
首次完整成功后才有容量和当前慢速代理积压 Gauge；失败保留旧值，年龄继续增长。
关闭开关也保留最后值；查看数据必须同时核对开关、available、年龄及刷新结果。
`busy` 说明没有空闲池连接，本轮立即跳过，不扩池或等待；
`limited` 表示候选超过 10000，整轮不发布截断统计；
timeout/storage/invalid_data 也只降级采集，不影响 readiness 或业务循环。
只读事务串行最多一个连接，总预算两秒，实际计划和耗时仍需另行授权核验。

复制并加载 `ops/monitoring/auction-recording.rules.yml`，
用安装版本的 `promtool check rules` 与 `check config` 核对后由用户重载 Prometheus。
`auction-alerts.rules.yml.template` 含占位，**不加入 rule_files**，不启用外部通知。
记录规则中的最后快照 Gauge 不使用 rate，新鲜度由年龄单独判断。

```promql
auction:sli_bid_http_p99_seconds:5m
auction:sli_realtime_push_p99_seconds:5m
auction:sli_bid_conflict_abort_ratio:5m
auction:sli_final_bid_retry_exhausted_since_start
auction:sli_final_bid_commands_per_second:last_snapshot
auction:sli_final_bid_peak_per_second:last_snapshot
auction:websocket_connections_peak:7d
auction_sli_pending{flow="slow_proxy_execution"}
auction_sli_flow_blocked{reason="awaiting_index"}
```

Final 耗尽原始累计非零即按已有 D5 条件提前评估 W12.1，不能只看 increase。
容量区分普通命令、代理创建、慢速代理，不把自动反击多条事实当多个用户命令。
WS 峰值是采样下界，数据库单秒峰值是已接受命令基线，均不是压力测试能力。
历史邮件、提醒、结算和慢速 tick 的候选索引已由迁移 077 补齐，但派生查询与证明核验尚未实现；
以 flow_available=0 和 awaiting_index 明示，不报告零延迟或 SLO 已达标。
Final 回帖导入已由 W0.5 接入；延迟上界、估计宽度和覆盖率须同时约束。
服务端响应、WS 写出、本地发送证明与付款观察均为代理，不是真实客户端收件时刻。
完整逐项状态和指标闭集见
[SLI owner 文档](../crates/auction-observability/README.md#sli-与容量基线)。

所有 SLO 与容量目标保持占位：**基线观察后由 Claude 提出、用户确认**。
回退汇总应在线明确 set `metrics_sli_snapshot_enabled=false`，沿用新命令 ID、
最新 revision、期限及原因规则；clear 只回到 TOML 基线，未必关闭。
已开始的短只读轮次可完成，之后不再查询；业务路径和已发布事实均不改动。
若回退旧程序，还须移除它不认识的 TOML 字段，旧程序会忽略未来数据库开关名。

### 运维告警

启用 `[worker.operations_alert]`（需要 MoogleMail 已启用且收件人在同一世界）后，Worker 定期扫描并通过 MoogleMail 发送告警邮件：投递积压（数量 / 最老年龄）、需要人工对账的投递、履约积压、付款逾期以及进程健康。同一信号有冷却时间（默认 30 分钟），通知按窗口合并。

### Admin 仪表盘

仪表盘集中展示：进程与 Worker 状态、各邮箱轮询、realtime 滞后与失败、凭据健康、投递/履约/恢复积压与告警状态。各分区独立降级。投递失败只统计仍待处置的失败：关注提醒的预检退役（从未开始发送）、Admin 对账判失败（以命令账本中产出当前版本的失败命令为证）、装备损失退役的原投递与已核验的 legacy 自寄修复不计入；履约失败不计入装备损失处置后转为操作者线下承担的买方退款。资金为“仅记账”时两个世界的恢复数据源都显示不适用，不读取外部余额。

### 日志

- 所有进程向 stdout 输出 JSON 日志（无 ANSI），由 journald 收集：`journalctl -u auction-worker -o cat | jq`。
- 过滤器由 `common.toml` 的 `log_filter` 控制（默认 `info,<服务名>=debug`），使用 `EnvFilter` 语法。
- 日志只包含安全分类与标识（命令 ID、拍卖/lot/义务 ID、错误码），不含连接串、Cookie、密钥、登录 key 或邮件正文。
- 可选 OTLP/HTTP tracing：在角色文件 `[tracing]` 启用，导出头只能写在敏感文件的 `*_otlp_headers` 中（进程环境中的 `OTEL_EXPORTER_OTLP_*HEADERS` 会导致拒绝启动）。

### 安全错误与未持久化指标

W0.2 已由 I-A（jobs/application）与 I-B（integrations/HTTP）共同覆盖。
新增日志 target 为 `auction::diagnostics`；先按 kind 选择 owner，再用 phase 定位步骤，
safe_kind 查看安全错误类别，按业务 UUID 或安全 span 关联同一义务/投递。
不同原因不修改持久化 defer reason/失败码，因此旧版本仍能读取、重放和恢复原事实。
kind 为简短 owner，phase 为语义步骤；同一二元组只表示同一种原因，禁止序号和模块路径标签。

kind 按业务 owner 汇总如下；日志 kind 与指标 kind 不同，后者始终是 safe_kind 闭集。
下表保留 I-A owner；后续表补齐 I-B 新 owner，复用的 owner 仍见本表。

| 业务 owner | 日志 kind |
| --- | --- |
| 管理用例 | `admin_intake`、`admin_runtime_config`、`admin_users`、`admin_worlds` |
| 竞价生命周期与慢代理 | `auction_lifecycle`、`slow_proxy` |
| 拍卖排期与唤醒提示 | `auction_schedule`、`auction_wakeup` |
| 竞价账本、历史与时间 | `competition_ledger`、`competition_history`、`competition_timestamp` |
| 履约调度与用例 | `settlement_fulfillment`、`fulfillment` |
| 履约授权与收件人 | `fulfillment_authorization`、`fulfillment_recipient` |
| 内部履约与退款 | `fulfillment_internal`、`fulfillment_refund` |
| 结算与结算账本 | `settlement`、`settlement_ledger` |
| 资金用例 | `funding`、`funding_record_only` |
| 投递与投递唤醒 | `delivery`、`delivery_wakeup` |
| 论坛主题同步与回复 | `forum_sync`、`forum_reply` |
| 论坛通知与私信 | `forum_notification`、`forum_pm` |
| 邮件入站与轮询 | `mail_intake`、`mail_poll` |
| 邮箱维护与只读观察 | `mail_maintenance`、`mail_observation` |
| 恢复派发与阶段推进 | `recovery_dispatch`、`recovery_resolution` |
| 恢复激活与买家黑名单 | `recovery_activation`、`recovery_blacklist` |
| 恢复健康与观察 | `recovery_health`、`recovery_observation` |
| 关注与偏好 | `lot_watch`、`lot_watch_preference` |
| 关注邮件与私信 | `lot_watch_mail`、`lot_watch_pm` |
| 公开黑名单与会话偏好 | `public_blacklist`、`session_preference` |
| 公开补偿场次与逐件明细 | `compensation_round`、`compensation_lot` |
| 公开补偿账本 | `compensation_movement` |
| Worker 控制面 | `control_listener`、`control_server`、`control_trigger` |
| Worker 运行时与实时消费 | `worker_runtime`、`realtime_consumer` |
| 运维告警 | `operations_alert` |


I-B 新增 owner：

| 范围 | 日志 kind |
| --- | --- |
| Admin 接口 | `admin_auctions` |
| Admin 接口 | `admin_audit` |
| Admin 接口 | `admin_credentials` |
| Admin 接口 | `admin_dashboard` |
| Admin 接口 | `admin_delivery` |
| Admin 接口 | `admin_forum` |
| Admin 接口 | `admin_lifecycle` |
| Admin 接口 | `admin_lots` |
| Admin 接口 | `admin_mail` |
| Admin 接口 | `admin_proxy_events` |
| Admin 接口 | `admin_realtime` |
| Admin 接口 | `admin_recovery` |
| Admin 接口 | `admin_reports` |
| Admin 接口 | `admin_rules` |
| Admin 接口 | `admin_runtime` |
| Admin 接口 | `admin_runtime_switches` |
| Admin 接口 | `admin_session` |
| Admin 接口 | `admin_settlement` |
| 备份适配器 | `backup_archive` |
| 备份适配器 | `backup_heartbeat` |
| 凭据与密钥 | `credential_check` |
| 凭据与密钥 | `credential_crypto` |
| 凭据与密钥 | `credential_login` |
| 装备来源与密钥 | `equipment_crypto` |
| 装备来源与密钥 | `equipment_diagnostics` |
| 装备来源与密钥 | `equipment_fetch` |
| 装备来源与密钥 | `equipment_link` |
| 论坛协议 | `forum_credentials` |
| 论坛协议 | `forum_forms` |
| 论坛协议 | `forum_profile` |
| 论坛协议 | `forum_session` |
| 论坛协议 | `forum_topic` |
| HTTP/实时传输 | `http_boundary` |
| HTTP/实时传输 | `http_listener` |
| HTTP/实时传输 | `http_readiness` |
| HTTP/实时传输 | `http_response` |
| HTTP/实时传输 | `http_transport` |
| 凭据与密钥 | `login_key` |
| 邮件协议与投递 | `mail_delivery` |
| 邮件协议与投递 | `mooglemail_baseline` |
| 邮件协议与投递 | `mooglemail_credentials` |
| 邮件协议与投递 | `mooglemail_diagnostics` |
| 邮件协议与投递 | `mooglemail_evidence` |
| 邮件协议与投递 | `mooglemail_identity` |
| 邮件协议与投递 | `mooglemail_inventory` |
| 邮件协议与投递 | `mooglemail_maintenance` |
| 邮件协议与投递 | `mooglemail_read` |
| 邮件协议与投递 | `mooglemail_return` |
| 邮件协议与投递 | `mooglemail_send` |
| 邮件协议与投递 | `mooglemail_session` |
| 邮件协议与投递 | `mooglemail_take` |
| Public 接口 | `public_account` |
| Public 接口 | `public_bid` |
| Public 接口 | `public_catalog` |
| Public 接口 | `public_compensation` |
| Public 接口 | `public_competition` |
| Public 接口 | `public_countdown` |
| Public 接口 | `public_history` |
| Public 接口 | `public_proxy` |
| Public 接口 | `public_realtime` |
| Public 接口 | `public_rules` |
| Public 接口 | `public_session` |
| Public 接口 | `public_worlds` |
| HTTP/实时传输 | `realtime_hub` |
| 运行时开关 | `runtime_switches` |
| 邮件协议与投递 | `settlement_delivery` |

HTTP 通过 `.fields.route` 查看路由模板，并在 `.span` / `.spans` 中关联规范化 request_id。
不要使用原始路径、query 或请求头过滤；未匹配与非请求路径使用固定占位类别。
例如筛选 Admin 运行时开关的数据库读取失败：

```bash
journalctl -u auction-admin-api -o cat |
  jq -c 'select(.target == "auction::diagnostics") |
    select(.fields.kind == "admin_runtime_switches") |
    select(.fields.phase == "read_switches") |
    {fields, span, spans}'
```

Sent 回退看 `mooglemail_baseline` 的 `fallback_before_send` / `fallback_after_send`，
用 safe_kind 区分锚点、页序、范围、预算或读取失败；影子计数不因日志抑制减少。
HTTP 就绪刷新看 `http_readiness`；Storage 的审计健康分类仍使用原 `auction::storage::audit_health`。

例如筛选内部履约的身份读取，并保留关联字段：

```bash
journalctl -u auction-worker -o cat |
  jq -c 'select(.target == "auction::diagnostics") |
    select(.fields.kind == "fulfillment_internal") |
    select(.fields.phase == "read_identity") |
    {fields, span, spans}'
```

可进一步使用 `.fields.safe_kind == "repository_failed"` 过滤。
不要依赖旧 message 文案或原错误字段；原日志统一改为安全字段后，人类可读消息仅作显示。
去重汇总位于 `auction_observability::log_dedup`，
包含固定 kind/phase/safe_kind 与 suppressed_count。
首条带原安全业务标识，汇总不保存标识。
Worker、Public、Admin 的长期服务 owner 均负责窗口刷新与关闭；HTTP 不计入 W0.3。
不要为了定位原因打开原错误正文、Cookie 或 URL 日志。
配置回退沿用 log_filter；禁用汇总 target 会放行原诊断日志。

新增 `auction_worker_unpersisted_error_total{loop,kind}` 表示没有确认对应持久处置的失败。
例如查询：

```promql
sum by (loop, kind) (increase(auction_worker_unpersisted_error_total[15m]))
```

它与轮次 error、持久重试计数分别解释，不能相加推断失败义务数。
提交未知计入但不授权重发；已持久化的退避、人工处置和正常排空不计入。
口径、关闭方式和 PM/实时/维护特殊情况见
[可观测性说明](../crates/auction-observability/README.md#未持久化错误)。

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

### Panic 排障

W13.3a 的 release 开启整数溢出检查，原生保留 `panic = "unwind"`。
溢出会失败关闭并进入已有 panic 边界，不再静默回绕；
它不证明业务已回滚，尤其不能据此重发外部写入。
WASM 继承开启检查，保留 abort，现有 hook 展示静态错误横幅。

先按 `auction::panic` target 定位线程与源码 `file`、`line`、`column`。
`payload` 只显示静态字符串，格式化 String 或其他类型均为固定占位；
顶层 join 日志只显示 `panic`、`cancelled` 或 `other`。
不要求采集完整 payload、凭据或外部响应来排障。
再查看 `auction_worker_task_panics_total{loop}` 的增量，
关联 `auction::diagnostics` 的 `phase=item_panic`、`safe_kind=task_panic`。
该指标按闭集循环预置零值，进程重启归零；
它只覆盖现有工作项子任务的 join 收口，不是工作项尝试次数。
原 `auction_worker_unpersisted_error_total` 的 `task_join` 口径保持不变。

生命周期、结算、慢速代理、履约、恢复 action 和 delivery 子项 panic
仍按原 `Canceled` 等结果合并，所属循环继续运行。
邮件、论坛、PM、告警叶子直接执行，没有新增 catch_unwind；
panic 仍退出常驻任务，Worker 随后排空并非零退出。
分体 Worker 模板由 systemd `Restart=on-failure` 拉起；
Public/Admin 的 Requires 绑定仍可能传播影响。
合并 server 当前会停止其余角色，模板 `Restart=no`，需人工核对后恢复。

工作项 panic 计次、封顶和人工阻断尚未实现：W13.3b 待用户决定，
原因是逐 owner 规则、所需迁移与影子框架超出本批无迁移范围。
当前恢复语义如下，不应把观测指标当成新的自动处置：

| 工作项 | panic 后的现状 |
| --- | --- |
| 生命周期 | 租约到期可重领，领取增加 attempt_count，无封顶 |
| 结算 | 重领不增加次数，只有显式失败落库才增加 |
| 履约、恢复 action | 重领不增加 retry_count，显式释放才增加 |
| delivery | claimed 可重领并计次；sending 到期加 grace 后转人工 |
| 邮箱轮询 | panic 不增加连续失败数；重启后可重领，成功清零 |
| 论坛 | 发布领取计次但无封顶；过期 claim 按未知写入对账 |
| PM | 发送前可重领；sending 到期转人工，只读对账不重寄 |
| 告警批次 | 已有三次上限，耗尽进入 failed/retry_exhausted |

普通 Public/Admin handler panic 由现有 HTTP 边界捕获，返回 500；
它不等同于整个角色退出，独立后台任务不在该请求边界内。
先核对持久状态和原命令结果，再决定人工操作；未知外写绝不自动重发。
构建回退仅恢复 release 的 overflow-checks 声明，并重建原生与 WASM 制品；
保留本批安全日志与指标，不改动任何已有工作项状态。

### SQL 失败诊断

先按既有命令、拍卖、lot 或投递标识关联失败日志。带安全上下文的错误在原中文原因后附加方括号字段：`sqlx_kind` 区分数据库错误与连接池、传输、解码等失败；`sqlstate_class` 为两位类别，`sqlstate` 为五位代码；`constraint`、`table` 可定位迁移中的约束、索引或表。字段规则的唯一说明见 [Storage SQL 错误安全上下文](../crates/auction-storage/README.md#sql-错误安全上下文)。

`none` 表示驱动未提供该字段，`invalid` 表示诊断字段格式异常；部分触发器错误没有约束名或表名，不能据此认定没有约束拒绝。结合制品/迁移版本、原业务上下文和持久状态排查，不收集驱动 message/detail/hint、SQL、绑定值、连接串或凭据。仅记录 `safe_kind` 的日志仍只有分类，并非每个失败入口都输出增强文本。

SQLSTATE 和错误文本用于定位原因，不能据此自动重试、重发邮件或认定提交已回滚；继续遵循原事务 owner 的类型化判定与未知结果对账流程。本项不新增配置，回退为换回旧制品；日志过滤器不能撤销已经写入日志或失败安全详情的增强文本。


## 运行时开关

已开放 `readiness_audit_failure_mode`（三角色）、
`readiness_cache_enabled`（Public/Admin）及
`metrics_sli_snapshot_enabled`（Worker，默认基线 false）、
`mooglemail_poll_isolation_enabled`（Worker，默认基线 true）和
`metrics_reject_forwarded_requests`（三角色，默认基线 true）。
通过 `auction-cli switch list` 读取 revision，再用稳定命令 ID、期望版本、
actor 与原因执行 set/clear，参数见[CLI 参考](../apps/auction-cli/README.md#运行时开关)。
切 classified 前仍须审阅[影子样本](#审计就绪影子观察)，系统不自动推进。
SLI 开关只控制附加只读汇总，不改变 readiness 缓存身份或业务授权。
邮件隔离开关控制 Worker 单世界的收件与维护执行路径；切换在下一轮调度
或下一次维护门复读后生效，在途 claim、租约、批次预算和 sweep 完整结束。
指标转发拒绝开关在每次 `/metrics` 请求的边界入口读取一次；业务路由与
三角色 loopback 出口共用同一动态策略。

`metrics_reject_forwarded_requests` 是安全策略，命令层只允许在线收紧：
`set true` 和 `clear` 成功，`set false` 固定拒绝并以业务运维退出码 5 失败。
放宽拒绝策略必须修改 common 的 `[metrics] reject_forwarded_requests` 并重启
对应角色；直接写库绕过 CLI/应用命令不在本批威胁模型内。

set 以未过期数据库事实覆盖启动时 TOML；clear 或到期只回到基线，基线不一定是 legacy 或 false。若要明确回旧路径，应分别 set legacy/false；若要恢复部署基线，则 clear。无期限覆盖需要显式 `--no-expiry`，会跨进程重启保留；修改 TOML 不会清除它。所有操作均追加事实与审计，期限自动回落不等待 Worker 的 expire 审计收口完成。

三角色启动即读，此后五秒刷新，每次一秒预算；正常切换在下一次成功刷新后供新检查使用。首次失败用基线，之后失败保留最后成功快照，原期限仍生效。Admin `/config` 的独立只读卡片显示数据库最新意图、版本、期限、操作者、原因及 Admin 自己的实际值与快照年龄，不依赖 runtime 文件配置。刷新卡片可比较数据库 revision 和 Admin 已观察 revision；该卡片不能替 Public 或 Worker 作应用确认。

通过三角色各自的受限指标入口核对 `auction_runtime_switch_refresh_successes_total`、`auction_runtime_switch_refresh_failures_total`、`auction_runtime_switch_snapshot_age_seconds`、`auction_runtime_switch_applied_revision` 和 `auction_runtime_switch_effective_value`。年龄为 -1 表示未成功；明显超过两个周期时按 stale 处理，核对进程、数据库连接、076 是否通过正式流程迁移，以及已知名称是否出现损坏值。关闭 exporter 不关闭刷新；没有指标时只能确认数据库意图和 Admin 本身，不能宣布其他角色已应用。日志只记录闭集角色、模式和安全状态，不回显秘密或原因正文。

审计分类累计不会因切换清零；HTTP 缓存绑定策略版本和期限状态，旧策略检查完成后不能回填当前缓存或返回旧成功。Worker control 原有健康口径不在本批统一，Worker 的审计模式应结合心跳及指标核对。

数据库 076 登记 compatible，但首次迁移仍受原停机流程约束；随后切换开关无需重启、
不受 W7.5 阻挡。程序回退须使用已含 W7.8a 且显式启用兼容模式的旧程序，按
[程序回退](deploy/RECOVERY.md#程序回退)核对超前区间；旧程序会忽略新开关并使用其
TOML。M1 Admin 只读；邮件隔离与指标转发已接入第二批，排空预算及迁移模式仍是
启动配置。W1.9 主动暂停尚未实现，不得将本机制视为已暂停外部请求。

## 数据库连接预算与启用顺序

先用 `auction-cli config check --strict` 查看 `database_budget` 表，
再决定是否启用服务超时。默认三服务池各 16，包含长期 owner；
Public 普通工作余量 15，Admin 为 16，Worker 最坏为 13。
维护池默认 4，预算允许两个 CLI，另预留 8 个专用/外部会话：
普通连接计划合计为 48 + 2 × 4 + 8 = 64。
声明 max_connections 为 100、superuser reserve 为 3、其他 reserve 为 0 时，
普通上限为 97，未分配余量为 33。
reserve 仍待用户核实线上；离线核对不执行 SHOW，也不发现其他部署实例。

预算的并行 CLI 数是操作约束声明，不是跨进程信号量。
备份的 SQLx 池与 pg_dump、重建 guard 与迁移池可能重叠，必须计入外部预留。
SQLx 借出并发与物理池容量不同；空闲池会话也消耗 PostgreSQL 连接名额。
增加实例、并行维护命令或 exporter 前，应重新核对预算。
字段和校验详见[数据库连接分档](config/README.md#数据库连接分档)。

启用步骤：

1. 默认 profiled，服务超时保持关闭；先完成指标采集与存储，观察完整场次。
2. W0.14 部署后按决策 72 观察 Final 与结算，差异仅在预期瞬时类别。
3. 用户将三角色有效 readiness 均切到 classified，确认无到期回落到旧模式。
4. 用户显式填写三个阈值后启用服务超时；候选为 acquire 2000ms、
   statement 5000ms、lock 1000ms。idle 默认 0，非零须另行决定。
5. 经 W7.5 停机门禁重启，使全部池和长期会话一致应用设置。
   本批不接入 W7.7；仅改运行时开关无法改写已借出的所有权连接。

回退先关闭服务超时并重启，再考虑将 readiness 切回 shadow/legacy。
`mode = "legacy"` 为连接完整旧路径：池 16、acquire 30 秒，无显式名称或会话设置。
重启同样受停机门禁约束。事务、事件、命令账本和未知外部结果处置不随模式改变。
