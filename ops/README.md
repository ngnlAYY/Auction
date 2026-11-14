# 运维手册

[docs/README.md](../docs/README.md) · [ops/config/README.md](config/README.md) · [ops/keys/README.md](keys/README.md) · [ops/deploy/RECOVERY.md](deploy/RECOVERY.md) · [apps/auction-cli/README.md](../apps/auction-cli/README.md) · [docs/architecture.md](../docs/architecture.md)

本手册覆盖生产部署、启停编排、升级、反向代理、监控与日常运维。常规运维通过 `auction-cli` 或 systemd 完成；普通服务启动从不隐式迁移、备份或删库。合并部署的首次设置在网页明确确认后，可由 `auction-server` 初始化预先建立的专用空库。

## 部署形态

| 形态 | 组成 | 适用 |
| --- | --- | --- |
| 分体部署 | Worker、Admin、Public 三个 systemd unit，`auction-stack.service` 调用 `auction-cli service` 按序编排 | 需要单独重启角色、细粒度资源限制或日后拆分主机 |
| 合并部署 | 一个 `auction-server.service`，以子进程运行三个角色 | 单主机、小规模、最少运维对象 |

两种形态共用同一配置布局与同一个 `auction-cli`，但不能同时用于同一套配置。

发布包 `auction-rust-<版本>-<提交前 12 位>[-single].tar.gz` 含 `bin/`、`docs/`、`systemd/`、`caddy/`、`config/examples/` 与 `retention-policy.v2.json`；分体包另含 `tmpfiles/`，合并包另含首次设置代理示例。敏感模板以 `secrets.toml.example` 等命名，须复制为配置引用的实际文件名；用包内 `SHA256SUMS` 校验解压文件。两种包均含定时备份与 WAL 模板，仅由用户按[RECOVERY](deploy/RECOVERY.md)单独启用；`docs/` 的 `MIGRATION-DRILL.md` 与回执模板用于含迁移部署。异机副本还须主机 `age`、离线密钥与 R2 配置，见[异机副本前置准备](deploy/RECOVERY.md#异机副本前置准备)。

## 目录与账号

| 路径 | 内容 | 建议属主与权限 |
| --- | --- | --- |
| `/opt/auction/bin/` | 程序 | root，0755 |
| `/etc/auction/` | 配置目录：`public.toml`、`admin.toml`、`worker.toml`、`common.toml` | 目录 root:auction 0750；文件 root:auction 0640 |
| `/etc/auction/secrets.toml` | 敏感配置 | auction:auction 0600 |
| `/etc/auction/runtime/admin.toml` | runtime 配置（Admin 写入） | 目录 auction:auction 0750；文件 auction:auction **0640** |
| `/etc/auction/maintenance/postgres.toml` | 集群 `postgres` 库维护凭据（可选，仅 `db create/rebuild`） | root 0600 |
| `/etc/auction/owner/postgres.toml` | 迁移 owner 凭据（`db create/migrate/rebuild`、`backup restore`、`service start/restart`）；常驻角色不得引用 | root 0600 |
| `/run/hv-auction/` | Public/Admin 的 Unix socket 与实例锁（tmpfiles 预建） | auction:auction 0750 |
| `/run/hv-auction/worker/` | Worker control socket（Worker unit 的 `RuntimeDirectory`） | auction，0750 |
| `/var/lib/auction/` | 部署控制锁与回执（`deployment/`、`deployment/receipts/`）、邮件诊断归档等可写数据 | auction / root |

```bash
useradd --system --home-dir /var/lib/auction --shell /usr/sbin/nologin auction
groupadd --system auction-worker-control
usermod -aG auction-worker-control auction
```

`auction-cli config check --strict` 检查配置目录与普通配置文件不能被 group/other 写入、敏感文件只能 owner 访问；runtime 文件在 loader 阶段就必须恰好 0640 且父目录禁止他人写入。`service start/restart` 还会核对 runtime 文件与父目录归 Admin unit 的固定 `User` 所有。

> 生产 `/etc` 对服务只读（`ProtectSystem=strict`），邮件诊断归档默认目录不可写。请在 `worker.toml` 把 `[worker.mooglemail.diagnostics] directory` 设为 `/var/lib/auction/mail-diagnostics` 这类可写的私有目录；runtime 配置不接管此路径。

## 首次部署

### 分体部署

1. **安装程序**：解压发布包，把 `bin/*` 安装到 `/opt/auction/bin/`；按上文建立 `auction` 账号与 `auction-worker-control` 组。
2. **准备数据库账号与数据库**（PostgreSQL 18）：迁移 owner 与服务运行账号必须分离，数据库 owner 是迁移 owner。

   ```sql
   CREATE ROLE auction_owner LOGIN PASSWORD '<owner 密码>';      -- 迁移 owner，持有 DDL
   CREATE ROLE auction_runtime LOGIN PASSWORD '<运行密码>';      -- 服务账号，迁移 081 授予只读+显式写入面
   ```

   手工 `CREATE DATABASE auction OWNER auction_owner`，或先写好配置再用维护凭据与 owner 凭据创建缺失的库（`db create` 不创建角色、不迁移）：

   ```bash
   auction-cli --config-dir /etc/auction db create --database auction \
     --maintenance-secrets /etc/auction/maintenance/postgres.toml \
     --owner-secrets /etc/auction/owner/postgres.toml
   ```

   三角色 `secrets.toml` 的 `[database].username` 填 `auction_runtime`，`common.toml` 的 `[database_roles] owner = "auction_owner"`。首次设置向导会试连数据库，须预先建好库与两个角色；向导写入的敏感文件只含运行账号，随后手工补 owner 凭据。合并部署初始化空库必须提供 `--owner-secrets`。模板见 [postgres-owner.toml](config/examples/secrets/postgres-owner.toml)。
3. **生成配置**，二选一：
   - 向导：先 `install -d -o auction -g auction -m 0750 /var/lib/auction`，再运行 `sudo -u auction /opt/auction/bin/auction-admin-api setup --output-dir /var/lib/auction/new-config --control-group auction-worker-control`（目录须尚不存在）。从 stderr 取得设置地址与一次性令牌，经 SSH 隧道（`ssh -L 8091:127.0.0.1:8091 <主机>`）完成表单，然后把目录内容移入 `/etc/auction/` 并调整属主与权限。
   - 手工：以 [ops/config/examples](config/examples) 为模板编写，字段说明见[ops/config/README.md](config/README.md)。
4. **补齐关键配置**：`secrets.toml` 的 `[database]`、`admin_actor`/`admin_secret`、三组独立数据密钥（`openssl rand -base64 32`）；`worker.toml [worker.control] enabled = true` 且 `allowed_peer_uids` 含 Admin UID 与 **0**；Public `[http_security]` 与 Admin runtime `[security]` 的 `allowed_origins`、`cookie_secure = true`，代理在本机时 `trusted_proxy_cidrs = ["127.0.0.1/32"]`；每个 Public 实例使用唯一的 `realtime.consumer_key`。
5. **安装运行时目录与 unit**（在解压后的发布包根目录执行）：

   ```bash
   install -m 0644 tmpfiles/hv-auction.conf.example /etc/tmpfiles.d/hv-auction.conf
   systemd-tmpfiles --create /etc/tmpfiles.d/hv-auction.conf
   for unit in auction-worker auction-admin-api auction-public-api auction-stack; do
     install -m 0644 systemd/$unit.service.example /etc/systemd/system/$unit.service
   done
   systemctl daemon-reload
   systemctl enable auction-stack.service     # 不要单独 enable 三个角色 unit
   ```

6. **检查并启动**：`auction-cli --config-dir /etc/auction config check --strict`；`systemctl start auction-stack.service`（等价于 `service start`：预检 → 迁移 → 按序启动并等待就绪），用 `service status` 确认三个角色均 ready。
7. **反代与凭据**：配置反向代理并登录 Admin；在“Cookie 池”或 `auction-cli credential import` 录入 MoogleMail 与论坛 Cookie，在 `worker.toml` 启用对应功能后 `service restart`。

### 合并部署

1. 安装 `auction-server` 与 `auction-cli`，建立 `auction` 账号，预先建立专用的**空**数据库与应用角色。
2. 安装 [auction-server.service.example](deploy/systemd/auction-server.service.example) 为 `/etc/systemd/system/auction-server.service`（配置目录 `/var/lib/auction/config`，运行目录 `/var/lib/auction/.config.server`），`systemctl enable --now auction-server`。
3. 配置目录不存在时进入首次设置：准备 owner 凭据 `/var/lib/auction/owner/postgres.toml` 并通过 `--owner-secrets` 传入；在 `journalctl -u auction-server` 取得令牌，经 SSH 隧道访问 `http://127.0.0.1:8091/setup`，确认“初始化并启动”。无法使用隧道时可临时用 [setup.Caddyfile.example](deploy/caddy/setup.Caddyfile.example) 反代并加 `--allow-remote-setup --setup-origin https://<域名> --setup-proxy-ip 127.0.0.1`，完成后立即删除。

初始化失败或结果未知时服务不会自动重试（unit 为 `Restart=no`）：保留配置与数据库现场，核对后用 `auction-cli db` 系列命令恢复。合并部署是单一故障域：三个子进程任一退出都会停止其余角色，需要人工核对后恢复。

## 端口与套接字

| 端点 | 默认 | 说明 |
| --- | --- | --- |
| Public HTTP | `127.0.0.1:8090` | `public.toml [listener]`；可改 Unix socket（`/run/hv-auction/`）。Caddy 上游默认该地址 |
| Admin HTTP | `127.0.0.1:8091` | `admin.toml`/runtime `[listener]`；首次设置也临时使用 8091 |
| Worker control | `/run/hv-auction/worker/control.sock` | Unix socket；只读 `/v1/health/live`、`/v1/health/ready`、`/v1/status`、`/v1/mail/progress`、`/v1/external-accounts`，peer UID/GID allowlist |
| Public/Admin Unix socket | `/run/hv-auction/`（`mode = "unix"` 时） | tmpfiles 预建 `auction:auction 0750`；listener 同目录放 `.public-api-listener.lock` / `.admin-api-listener.lock` 实例锁 |
| 角色指标出口（可选） | `127.0.0.1:19100/19101/19102` | 仅 `metrics.enabled` 且填写 address 时监听；示例值，可改但不可重复 |
| PostgreSQL | `127.0.0.1:5432` | 只监听本机或 Unix socket，见[ops/postgres/README.md](postgres/README.md) |
| Caddy | `80/443` | 唯一对公网端口；`/metrics` 固定 404 |
| Prometheus / node / postgres exporter | `127.0.0.1:9090/9100/9187` | 仅本机；经 SSH 隧道查看 |

`ss -ltnp` 应只看到上表预期端口；出现 `0.0.0.0`/`::` 的 8090/8091、19100–19102、5432、9090、9100、9187 都要按[防火墙与端口](hardening.md#防火墙与端口)处理。

## systemd 服务与顺序控制

| unit | 说明 |
| --- | --- |
| `auction-worker.service` | `Restart=on-failure`；`RuntimeDirectory=hv-auction/worker` 管理 control socket 目录；可写 `/var/lib/auction` |
| `auction-admin-api.service` | `Wants=`/`After=` Worker；附加组 `auction-worker-control` 以访问 control socket；可写 `/etc/auction/runtime` |
| `auction-public-api.service` | `Wants=`/`After=` Worker 与 Admin |
| `auction-stack.service` | oneshot、`RemainAfterExit`，以 root 执行 `auction-cli service start/stop`；开机按序拉起 |
| `auction-server.service` | 合并部署专用；`KillMode=mixed`、`TimeoutStopSec=225s`、`Restart=no` |

Public/Admin unit 保持 `TimeoutStopSec=45s`，Worker 为 `120s`，stack 为 `330s`。外层上限按“普通 + 关键段 + tracing 上限 + 余量”计算：普通预算默认 10 秒，Worker 关键段默认 60 秒，tracing 上限 30 秒，supervisor 默认 195 秒；提高应用预算须同步提高 unit 上限，详见[排空与停止预算](config/README.md#排空与停止预算)。

**故障域隔离**：Public/Admin 对 Worker（Public 还对 Admin）只用 `Wants=` 与 `After=`，不用 `Requires=`。Worker 退出并重启时 Public 与 Admin 继续响应，依赖 Worker 的操作可能暂时失败，恢复后不重放外部写入；readiness 编排由 `auction-cli service` 负责。Worker 停止时取消尚未登记交接的准备期工作并保留持久 claim：delivery、PM 与 compose discard 在持久围栏前登记，关闭先赢时不发起围栏；已登记交接与关键段共享一次 60 秒截止，之后普通收口最多 10 秒，仅在既有结果端口确认提交后 complete。`deploy/rollback` 停机前执行在途发送门禁；直接 systemd 停止仍可能耗尽预算，届时按人工对账或只读证明处理，绝不自动重发。

**为什么用 `auction-cli service` 编排**：systemd 的 `After=` 只保证进程已启动，不保证就绪。`service start` 在每个角色启动后等待 readiness（Worker 为 control socket `/v1/health/ready`，Admin/Public 为 `/health/ready`，期间 `MainPID`/`InvocationID` 不变）才启动下一个；任一步失败反序停止本次启动的 unit，并在启动前执行严格配置检查与迁移。详见[分体部署编排](../apps/auction-cli/README.md#分体部署编排)。

```bash
auction-cli --config-dir /etc/auction service status     # 状态与 readiness（未全部就绪时退出码 4）
auction-cli --config-dir /etc/auction service restart    # 预检 → 时段门禁 → 反序停止 → 迁移 → 顺序启动
auction-cli --config-dir /etc/auction service stop       # 时段门禁 → 按 Public → Admin → Worker 停止
```

三个角色 unit 的 `Wants=`/`After=` 不提供应用 readiness 编排；整体重启用 `service restart`。Worker 在凭据所有权失效、control 停止等运行时故障时非零退出并由 `Restart=on-failure` 拉起。迁移、建库、重建与恢复要求三个 unit 全部停止，见[停机门禁](../apps/auction-cli/README.md#停机门禁)。

### 停机时段保护

`service stop/restart` 在停机前只读检查数据库：任一场次处于 Final，或任一在拍 lot 的有效截止不晚于未来保护窗口末端（含已过截止但仍为 live），就拒绝并以 4 退出，输出场次/lot ID 与截止时间；慢速代理按同一截止条件计入；数据库不可达或查询失败同样默认拒绝，`start` 不受影响。

保护窗口用 `--protection-window-minutes` 设置（默认 60，1–10080），须按[停机时长与窗口](deploy/MIGRATION-DRILL.md#停机时长与窗口)设为实测停机时长的 3 倍。紧急例外用 `service stop/restart --force --reason '具体原因'`（缺一即参数错误，退出码 2），原因不可含秘密或玩家信息；已过截止仍为 live 的 lot 须先排查生命周期故障再记录原因强制停机。只读快照不冻结状态：直接 systemctl 停止、合并部署停机、自动更新重启、关机与断电都可能绕过 CLI。无人值守自动重启须按[无人值守升级与重启窗口](hardening.md#无人值守升级与重启窗口)关闭或排期。

## 主机加固基线

主机层基线（systemd 资源上限与沙箱、SSH、防火墙、PostgreSQL 监听、无人值守升级窗口、时间同步、journald）见 [ops/hardening.md](hardening.md)：模板随发布包提供，仓库不改主机服务；线上 unit 名可能是 `auction-test-*`，须把指令合并进实际 unit 后 `daemon-reload`，再按 `service restart` 受控生效。

## 升级

**只部署 GitHub Release 资产。** 从 `main` 上 `v<版本>` tag 产生的 Release 下载 `*.tar.gz`、`SHA256SUMS` 与 `DEPENDENCIES.md`，部署前运行 `bash ci/release/verify-deployment-package.sh <归档> --tag <v版本>`（或 `just verify-package …`）核对摘要、包内清单、构建提交可从 tag 到达与来源证明。普通 CI 候选 artifact、本地 `just release-check` 产物与 `-dirty` 包均不可部署；未通过校验的包不得安装、停止服务或迁移。

分体部署用发布包内的 `auction-cli deploy` 完成严格预检、时段与在途发送门禁、演练回执核对、停机备份与新旧 CLI 校验、保留旧程序、原子安装、迁移与顺序启动、运行摘要复核并追加回执。含迁移的版本必须先用候选制品完成[迁移演练](deploy/MIGRATION-DRILL.md)并传入回执：

```bash
/opt/auction-run/<发布包>/bin/auction-cli --config-dir /etc/auction deploy /opt/auction-run/<发布包> \
  --owner-secrets /etc/auction/owner/postgres.toml --backup-dir /var/backups/auction \
  --migration-drill-receipt /srv/auction-drill/receipts/<drill>.json
# 停机前查看计划
... deploy /opt/auction-run/<发布包> --dry-run
```

命令必须在制品目录内运行该包的 CLI（迁移集合与期望 schema 来自它）。`--dry-run` 不备份、不停机、不安装、不写回执。主要门禁与默认值：`--drill-max-age-days` 7、`--send-gate-timeout-seconds` 30、`--startup-timeout-seconds` 30、`--protection-window-minutes` 60。含迁移时回执须为 7 天内、由当前库版本恰好迁移到候选版本、与本次制品提交/包名/CLI 摘要一致、三份 readiness 与一致性巡检通过，且保护窗口不小于回执记录下限。

**失败与未知结果**：

- 停机前失败只输出错误，不停止、不安装、不写回执。
- 停机后失败追加 `action=deploy`、`outcome=failed` 回执并退出非零；命令不自动回滚或重启，用 `service status` 确认实际状态。启动阶段失败的回执带 `rollback_record_id`，可执行 `rollback --receipt-id`。
- 迁移已提交但角色未就绪：先看 `service status` 与 journal 修配置，再用 `service start`（不重复迁移）；回到旧程序只能走 `rollback`（区间全为 `compatible`）或恢复停机备份。
- 停机备份、回执或安装失败时没有可信恢复点，按[RECOVERY](deploy/RECOVERY.md)核对 catalog 与程序摘要后再决定；不要手工覆盖 `/opt/auction/bin/` 后直接启动。

部署后核对（任一项不符按失败处理）：`service status` 三个 unit 的 `MainPID`/`InvocationID` 与 readiness 均为 ready；`service receipt show <id>` 的 `outcome=succeeded`、四个程序摘要与发布包一致、迁移版本等于期望、备份指向本次停机备份、演练回执已审核；`db status` 与服务内嵌版本一致且无待执行迁移；`/proc/<MainPID>/exe` 摘要与回执一致。

手工流程（`deploy` 不适用时）：新配置先 `config check --strict`，含迁移版本完成演练并审核回执；`service stop` 后 `backup create` 并记下恢复点；保留旧程序、安装新程序；`service start --owner-secrets … --backup-id <恢复点>` 完成预检、迁移、按序启动与回执；`service status` 与 Admin 仪表盘确认。无迁移的小版本可直接 `service restart --owner-secrets …`。部署历史以状态目录回执为准；如需随备份归档，把 `[backup.scope].deployment_receipts_dir` 指向回执目录。

合并部署：`systemctl stop auction-server` → 备份 → 安装新程序 → `sudo -u auction auction-cli --config-dir /var/lib/auction/config db migrate --owner-secrets …` → `systemctl start auction-server`。迁移 owner 凭据只供该命令与首次设置使用。

历史迁移的强制约束：迁移 `000069` 后新建场次使用统一加价与代理上限出价；迁移 `000071` 把两个世界的补偿池合并为全局池并冻结旧表（见[database 契约](../docs/contracts/database.md#拍卖与竞价)与[补偿池](../docs/guide/settlement.md#补偿池)）。两类迁移都必须停止全部旧角色、备份、迁移后同版启动。

迁移 `000108` 起出站通知模板保存在数据库（见[ops/config/README.md#出站通知模板数据库](config/README.md#出站通知模板数据库)）。升级含该迁移的版本时注意：升级后所有模板先使用内置默认正文，runtime 文件里固化的旧正文不再生效（区块原样保留，只为回退时旧程序仍能加载）；需要沿用的定制正文在升级后到 Admin 参数页的“MoogleMail 模板”重新保存。`worker.toml` 若带有 `[worker.mooglemail.mail_templates]` 须先删除，否则 `config check` 与 Worker 启动失败。该迁移登记为 `compatible`，回退到旧程序后旧程序重新使用自身配置里的模板。

**回退**：迁移只向前。`auction-cli rollback --receipt-id <deploy_receipt_id>` 只换回 `deploy` 保留的旧程序、不执行迁移，停机前核对安装目录、`previous_version → 当前版本` 的迁移均连续且登记为 `compatible`，并重跑时段与在途发送门禁；安装前把当前程序保留成新的回滚记录。含 `breaking`、缺登记或无法确认时必须恢复升级前备份；`service restart` 等手工回退还须显式设置 `[readiness] migration_compatibility_mode = "compatible"` 并保留新版 CLI。合并部署没有 CLI 回退：换回旧的 `auction-server` 程序、保留新版 CLI，按[程序回退](deploy/RECOVERY.md#程序回退)核对区间后启动。

## 反向代理

[ops/deploy/caddy/Caddyfile](deploy/caddy/Caddyfile) 是 Public 与 Admin 站点模板：自动 HTTPS；`/api/v1/ws` 由 `reverse_proxy` 透传；对公网屏蔽 `/metrics`（404）。上游默认 `127.0.0.1:8090` 与 `8091`，可改 Unix socket（同步 listener 的 `mode`、`trusted_unix_proxy` 与组权限）。服务端只采信 `trusted_proxy_cidrs`（或受信 Unix 上游）的 `X-Forwarded-For`。HTTPS 只在 Caddy 或前置 CDN 终止；HSTS 依赖 `X-Forwarded-Proto`，不要在 `header_up` 固定为 `http`。建议 Admin 只对内网或额外认证开放。

模板的几项约定：

- **客户端地址**：全局 `servers` 段把 CDN 的回源地址段列为可信代理，并按 `CF-Connecting-IP`、`X-Forwarded-For` 解析真实客户端；`reverse_proxy` 只把解析后的单个地址写入 `X-Forwarded-For`。可信代理不是访问白名单，只决定是否采信转发头。Cloudflare 地址段变化时同步列表；不经 CDN 的部署可删除该段。
- **压缩**：入口文档与静态资源全部压缩（带指纹的资源由后端提供构建期 brotli 副本，Caddy 只兜底）。接口只压缩后端标了新鲜度（`max-age`）的公开只读响应，让源站到 CDN 这一段不再传未压缩的 JSON；带会话、身份或命令结果的响应是 `no-store`，不压缩。压缩后的 ETag 带 `-gzip`/`-zstd` 后缀，Caddy 在转发条件请求前去掉后缀，后端的 304 再验证不受影响。
- **缓存头**：Caddy 不改写任何缓存头，浏览器与 CDN 的策略完全来自后端（见[docs/contracts/routes.md#缓存头](../docs/contracts/routes.md#缓存头)）。
- 修改后用 `caddy validate --config <文件> --adapter caddyfile` 校验，再 `systemctl reload caddy`。线上文件若另有监控站点等本机专属内容，按模板逐段合并，不要整文件覆盖。

### CDN（Cloudflare）

Cloudflare 默认只按**文件扩展名**决定能否缓存：`.js`、`.css`、`.svg` 等在默认名单里，`.wasm`、HTML 与 JSON 不在，后者无论源站发什么缓存头都是 `cf-cache-status: DYNAMIC`。因此只靠默认行为时，体积最大的 WASM 每次都回源，所有接口也都回源。后端已经为每个响应单独下发 `Cloudflare-CDN-Cache-Control`（默认 `no-store`，只有明确批准的响应才是可缓存指令），所以可以放心把缓存资格交给规则、把“存不存、存多久”交给源站。

在 Caching → Cache Rules 建两条规则（先匹配先生效，Admin 放前面）：

| 顺序 | 匹配 | 设置 |
| --- | --- | --- |
| 1 | 主机名等于 Admin 域名 | Cache eligibility：Bypass cache |
| 2 | 主机名等于 Public 域名 | Cache eligibility：Eligible for cache；Edge TTL：Use cache-control header if present, bypass cache if not；Browser TTL：Respect origin |

第 2 条生效后各类响应的预期状态：

| 响应 | 源站的 CDN 指令 | `cf-cache-status` |
| --- | --- | --- |
| `/assets/*`（JS、WASM、CSS、译文、图标） | `public, max-age=31536000, immutable` | 首次 `MISS`，之后 `HIT` |
| `/favicon.ico` | `public, max-age=86400` | `HIT`，过期后 `REVALIDATED` |
| 入口文档（`/` 与各页面路径）、补偿池汇总（含当前余额） | `no-cache` | `REVALIDATED`：每次向源站做条件请求，未变化时源站只回 304，正文由边缘给出 |
| 低频内容：世界目录、规则、关于、黑名单、注册说明、已结算拍卖列表、价格历史、补偿池流水与明细 | `public, max-age=300, stale-while-revalidate=3600` | `HIT`；过期后一小时内 `UPDATING`/`STALE`（先回旧值，后台刷新） |
| 已结算批次的 lots、lot 详情与竞价日志 | `public, max-age=86400, stale-while-revalidate=604800` | 同上 |
| 倒计时图片（仍倒数或未排期） | `public, max-age=5` | `HIT`，5 秒后 `EXPIRED` |
| 倒计时图片（计时目标已过去） | 低频档（5 分钟，`stale-while-revalidate` 1 小时） | `HIT`，过期后先回旧图再后台刷新 |
| 进行中场次的读取、会话、`/api/v1/me/*`、论坛同步开关与镜像、就绪检查、全部写入与错误 | `no-store` | `BYPASS` 或 `DYNAMIC` |

进行中场次的读取不进边缘缓存是刻意的：竞价事件之后客户端立即再验证，边缘上哪怕 2 秒的旧副本也会把它答成 304。只要源站指令是 `no-store`，规则再宽也不会缓存它；不要用“Ignore cache-control header and use this TTL”覆盖 `/api/*`。

低频内容的可见延迟：在管理端发布规则或关于、增删黑名单、启停世界，或场次结算之后，公开端最迟在“5 分钟后的第二次访问”看到新内容，单次访问看到的内容最多旧约 65 分钟。这些内容只是公示，准入与金额由服务端即时判定，不受缓存影响。需要立即生效时，在 Caching → Configuration → Custom Purge 按 URL 清除对应接口（如 `https://<Public 域名>/api/v1/rules?lang=zh-CN`；带查询串的接口每个查询串是独立的缓存项），或按前缀 `/api/v1/` 清除。

其余设置：

- **Rocket Loader：建议关闭**（Speed → Optimization → Content Optimization，或对 Public/Admin 主机名建 Configuration Rule）。它会把入口文档里的模块脚本改写成自己的类型、等自身加载器执行后才启动应用，对纯 WASM 的单页应用只增加一次往返和延迟；它与其他会注入内联脚本的功能还会被本站 CSP（不允许内联脚本）拦截。构建时 `ci/release/web-bundle.sh` 已给入口模块脚本加上 `data-cfasync="false"`，即使 zone 级开着 Rocket Loader 也不会改写应用脚本；关闭它只是省掉一次无用的 `rocket-loader.min.js` 注入。
- **Early Hints：开启**（Speed → Optimization → Protocol Optimization）。入口文档带 `Link: rel=preload` 响应头，Cloudflare 记住后会在等待源站应答入口文档期间先发 103，浏览器提前取回 WASM、脚本与样式。
- **Tiered Cache：开启 Smart Tiered Cache**（Caching → Tiered Cache）。访问量小时单个节点很容易没有副本，开启后节点未命中先问上层节点而不是回源。
- **Browser Cache TTL：Respect Existing Headers**；缓存键保持默认（包含查询串），不要启用忽略查询串。
- **SSL/TLS：Full (strict)**。
- 不需要为静态资源做清除：文件名带内容指纹，入口文档每次再验证。**升级改变了公开只读接口的响应形状时**，部署后在 Caching → Configuration 做一次 Purge Everything（或按前缀 `/api/` 清除）：边缘上旧形状的已结算/历史响应最长可保留一天，新页面的严格解码会拒绝它们。

配置后的核对（把域名换成实际值；`.wasm` 的文件名取自入口文档）：

```bash
curl -sI https://auction.example.com/ | grep -iE 'cf-cache-status|^link'
curl -sI -H 'Accept-Encoding: br' https://auction.example.com/assets/<名称>.wasm | grep -i cf-cache-status   # 第二次起为 HIT
curl -sI https://auction.example.com/api/v1/worlds | grep -iE 'cf-cache-status|cdn-cache-control'            # 第二次起为 HIT
curl -sI 'https://auction.example.com/api/v1/auctions/current?world=main' | grep -i cf-cache-status           # 始终 BYPASS/DYNAMIC
curl -s https://auction.example.com/ | grep -c rocket-loader                                                  # 关闭后为 0
```

命中率的上限由请求构成决定：就绪检查、会话与进行中场次的读取必须回源，它们在访问量小的时候占请求的大多数；上述配置解决的是“该缓存的没有缓存”（WASM、入口文档、世界目录与已结算/历史内容）。

## 监控与日志

指标名、label 闭集与口径由[crates/auction-observability/README.md](../crates/auction-observability/README.md)拥有；本节只保留安装、启用与核对命令。

### 健康检查

Public/Admin：`GET /health/live`、`GET /health/ready`（数据库按迁移模式校验、审计门禁通过；Public 另要求 realtime relay 新鲜；默认一秒进程缓存）。Worker：control socket 的 `GET /v1/health/ready`（与 Public/Admin 同一数据库/审计模式判定），例如 `curl --unix-socket /run/hv-auction/worker/control.sock http://localhost/v1/health/ready`（调用者须在 peer allowlist 中）。分体部署用 `auction-cli service status` 汇总。三个角色每 10 秒写心跳（TTL 30 秒），Admin 仪表盘将过期心跳显示为缺失；`/v1/external-accounts` 只是只读诊断快照，不参与就绪或审计裁决。

### 指标

Public/Worker 的 `metrics.enabled` 来自 common；Admin 使用 runtime 时只有 enabled 由 runtime 覆盖，地址与拒绝策略仍来自 common。附加出口只挂载 `/metrics`，使用同一注册表；默认对带识别转发头的任意方法返回空 404（拒绝优先于 request-id 400），Worker control 仍按 UID/GID allowlist 授权；绑定或服务失败只降级采集（`metrics_bind_failed` 等），scrape 不查询数据库或外部服务。Caddy 的公网 `/metrics` 404 规则继续保留，采集不经过反代或 Cloudflare。完整字段见[commontoml](config/README.md#commontoml)。

#### Prometheus 与 exporter（用户安装）

以下安装由用户执行，仓库静态门禁不代替；模板按约 1.9 GB 内存、有限磁盘的小型单机取值，先保留 15 天。

1. **固定版本与账号。** 安装 Prometheus、node_exporter、postgres_exporter，记录来源与版本；建立系统账号 `prometheus`、`node_exporter`、`auction_metrics`。数据库角色由 DBA 执行（库名换成实际目标）：

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

   不授予应用表读写、对象所有权或整个 `pg_monitor`；查询只导出聚合。优先 Unix socket peer 认证（`pg_hba.conf` 允许 `local auction auction_metrics peer`），密码只放私有凭据文件。
2. **配置与 unit。** 从仓库根复制并核对路径、用户与权限；`postgres-exporter.env` 设 root owner、`auction_metrics` group 与 0640：

   ```bash
   install -d -m 0755 /etc/prometheus
   install -m 0644 ops/monitoring/prometheus.yml.example /etc/prometheus/prometheus.yml
   install -m 0644 ops/monitoring/auction-recording.rules.yml /etc/prometheus/auction-recording.rules.yml
   install -m 0644 ops/monitoring/postgres-queries.yml.example /etc/prometheus/postgres-queries.yml
   install -m 0644 ops/monitoring/prometheus.service.example /etc/systemd/system/auction-prometheus.service
   install -m 0644 ops/monitoring/node-exporter.service.example /etc/systemd/system/auction-node-exporter.service
   install -m 0644 ops/monitoring/postgres-exporter.service.example /etc/systemd/system/auction-postgres-exporter.service
   ```

   自定义查询须按安装版本的配置接口接入；用 `promtool check config` 核对后 `systemctl daemon-reload` 并启用三个监控 unit（与 Auction 角色没有 Requires 关系）。
3. **启用出口。** 在 common 开启 metrics 并填三个地址（Public `127.0.0.1:19100`、Admin `19101`、Worker `19102`）；有 Admin runtime 时另设 `metrics.enabled = true`。重启后 `ss -ltnp` 确认只绑定 `127.0.0.1`。
4. **采集核对。** 在部署主机直接请求各 `/metrics`（不附转发头），确认 `up{job=~"auction-public|auction-admin|auction-worker|node|postgres"} = 1`，且 exporter、WAL 大小、autovacuum、长事务与锁等待采集持续更新。SSH 隧道 `-L 19090:127.0.0.1:9090` 查看；`auction-alerts.rules.yml.template` 含占位，**不加入 `rule_files`**。采集失败先核对二进制路径、`EnvironmentFile` 权限与 peer 认证，不要把指标端口暴露到公网。回退：删除对应 `*_loopback_listen_address` 并重启角色，或关闭 `metrics.enabled`；转发拒绝须改 common 的 `reject_forwarded_requests = false` 并重启。

### 审计就绪影子观察

默认 `[readiness] audit_failure_mode = "shadow"`、`cache_enabled = true`：shadow 保持旧就绪判定及心跳全部失败累计，同时在数据库检查成功后比对审计门禁；classified 只由 permanent + unknown 失败阻断，须经影子观察后由用户显式切换；`legacy` 仍可选。分类字典见[crates/auction-storage/README.md#审计失败与就绪](../crates/auction-storage/README.md#审计失败与就绪)。

1. 先确保指标采集可见：Public/Worker 取 common，Admin 有 runtime 时另查开关，Worker 从 control socket 抓取。
2. 用户把 common 基线改为 `shadow` 并受控重启，或在线设置 `readiness_audit_failure_mode=shadow`。日志 target `auction::storage::audit_health` 只在 `log_filter` 追加 `,auction::storage::audit_health=warn` 时输出分类（关闭用 `=off`）。
3. 用下列 PromQL 核对：影子期差异只允许 transient，permanent/unknown 必须为零，且必须有对应分支样本；无样本不算通过，不得为取样连接生产库或制造故障。
4. 审阅证据后在线切 `classified`；修改 common 基线仍需受控重启，系统不自动切换。
5. 回退：set `shadow` 或 `legacy`、`readiness_cache_enabled=false`；clear 只回 TOML 基线（默认 shadow）。不提供清零接口或试探审计。

```promql
sum by (instance, class, sqlstate_class, kind) (auction_audit_write_failures_total)
sum by (instance, class) (increase(auction_readiness_shadow_mismatch_total[1h]))
sum by (instance, branch) (increase(auction_shadow_evaluations_total{owner="audit_readiness"}[1h]))
sum by (instance) (increase(auction_shadow_mismatch_total{owner="audit_readiness"}[1h]))
auction_audit_readiness_blocked
sum by (instance, outcome) (rate(auction_readiness_cache_requests_total[5m]))
sum by (instance, result) (rate(auction_readiness_checks_total[5m]))
histogram_quantile(0.95, sum by (instance, le) (rate(auction_readiness_check_duration_seconds_bucket[5m])))
```

### Sent 发送基线影子观察

默认 `[worker.mooglemail] sent_baseline_mode = "shadow"`：旧完整排除集唯一裁决，水位分支复用同一读取轮次的页面，不执行额外 POST。四族指标口径见[crates/auction-observability/README.md#sent-发送基线局部影子指标](../crates/auction-observability/README.md#sent-发送基线局部影子指标)。

1. 保持 shadow，按 world 与 branch 核对四族样本与共享框架样本；零值或无样本不算通过。
2. Sent MID 语义的浏览器只读核实需另行授权，分别查看 main/isekai；只读 Sent 列表，不打开未读邮件或登录 key 邮件，不为采样发信或删除邮件。
3. 观察 7–14 天并至少覆盖一个含 Final 与结算的完整场次；正常可比较样本差异必须为零，超时/取消/窗口不一致样本单列。`cache_boundary_delta`、`invalid_mid/order_violation/anchor_changed/scope_changed/response_hint_conflict` 等前提异常未解释时不得切换。
4. 通过后由用户把 TOML 改为 `watermark` 并受控重启（不接入在线开关）；回退 `full`。完整扫描仍有 100 页上限，找不到 Sent 不等于未发送，绝不自动重发；换回旧二进制前移除其不认识的新字段。

```promql
sum by (instance, world, branch) (increase(auction_mooglemail_sent_baseline_shadow_evaluations_total[24h]))
sum by (instance, world, class) (increase(auction_mooglemail_sent_baseline_shadow_mismatch_total[24h]))
sum by (instance, world, reason) (increase(auction_mooglemail_sent_baseline_shadow_uncomparable_total[24h]))
sum by (instance, world, phase, reason) (increase(auction_mooglemail_sent_baseline_fallback_total[24h]))
```

### SLI 基线与容量观察

`[metrics] sli_snapshot_enabled` 默认 false，Admin runtime 不覆盖；TOML 基线须启动加载，已有有效 metrics 后可由用户在线设置：

```bash
auction-cli --config-dir /etc/auction switch list
auction-cli --config-dir /etc/auction switch set metrics_sli_snapshot_enabled true \
  --command-id <稳定UUID> --expected-revision <list中的版本> \
  --no-expiry --actor <操作者说明> --reason <非敏感原因>
```

数据库覆盖在下一次五秒刷新应用，汇总按六十秒唤醒，因此还要等待下一轮成功快照；默认禁用不发 SLI SQL，metrics 未启用或无数据库时不建任务。用 `auction_runtime_switch_effective_value{role="worker",switch="metrics_sli_snapshot_enabled"}`、`auction_sli_snapshot_enabled`、`auction_sli_snapshot_available`、`auction_sli_snapshot_age_seconds`、`auction_sli_snapshot_refreshes_total` 核对：available=0、age=-1 表示尚未成功，不能当作业务延迟为零；关闭开关也保留最后值。`busy` 表示没有空闲池连接、`limited` 表示候选超过 10000（整轮不发布截断统计），timeout/storage/invalid_data 只降级采集。完整口径见[crates/auction-observability/README.md#sli-与容量基线](../crates/auction-observability/README.md#sli-与容量基线)。

复制并加载 `ops/monitoring/auction-recording.rules.yml`，用 `promtool check rules` 与 `check config` 核对后由用户重载。`auction-alerts.rules.yml.template` 含占位，**不加入 rule_files**。记录规则：

```promql
auction:sli_bid_http_p99_seconds:5m
auction:sli_realtime_push_p99_seconds:5m
auction:sli_bid_conflict_abort_ratio:5m
auction:sli_final_bid_retry_exhausted_since_start
auction:sli_final_bid_commands_per_second:last_snapshot
auction:sli_final_bid_peak_per_second:last_snapshot
auction:websocket_connections_peak:7d
auction_sli_pending{flow="slow_proxy_execution"}
sum by (flow, world) (auction_sli_window_samples)
```

Final 耗尽原始累计非零即提前评估同场热点改造。WS 峰值是采样下界，数据库单秒峰值是已接受命令基线，都不是压力测试能力；历史邮件、提醒、结算与慢速 tick 的派生窗口以 unknown/incomplete 与 flow_available 明示，不报告零延迟或 SLO 已达标。所有 SLO 与容量目标保持占位，基线观察后由用户确认；回退在线 set false，clear 只回 TOML 基线。

### 运维告警

`[worker.operations_alert]` 的扫描与主动通知已拆分：`scan_enabled`（默认 true）让 Worker 定期扫描并写入告警台；`notify_enabled`（默认 false）才通过 MoogleMail 发送告警邮件，启用时还需要 MoogleMail 已启用且收件人在同一世界。Worker 扫描九个来源：`process`、`delivery`、`fulfillment`、`recovery`、`mail`、`credential`、`lifecycle`、`forum`、`notification`；`reconciliation` 由资产对账报告产生。

主机侧另有两个来源由 `auction-cli alert report` 报告（timer 示例见 `ops/deploy/systemd/auction-alert-report.{service,timer}.example`）：`backup`（备份目录容量低于 `[backup]` 水位、最近一次定时备份终态不是 `completed`）与 `wal`（WAL 暂存段超过 30 分钟仍未上传）。Worker 无法读取主机文件系统，因此不扫描这两个来源；同一事实重复提交是幂等 upsert。

同一信号有冷却时间（默认 30 分钟），通知按窗口合并。需要立即处理的种类沿用冷却后重复通知；付款逾期、待重列、队列延迟、论坛主题发布失败与非结算通知失败属于自动重试/自动推进的等待类，24 小时内最多合并为一封每日摘要。观察窗口是代码常量：收件轮询停滞与生命周期宽限各 15 分钟、队列延迟 10 分钟、账号持续忙 15 分钟、WAL 暂存积压 30 分钟；Sent 与论坛远端页数在达到该次扫描观察携带的有效上限的 80% 时告警。告警种类、来源与范围是数据库 CHECK 闭集（涉及迁移 005、015、020、085、091、095、099、101 等），新增来源必须随迁移扩展并按迁移兼容登记规则声明等级。每个来源的处置步骤见[告警处置手册](#告警处置手册)。

### 告警台与 Admin 仪表盘

告警台在 `scan_enabled = true` 时始终可用，列出活动与历史告警（`active`/`resolved`/`all`），表头锚点对应上文处置表。确认只追加操作者事实；静默只把重复通知推到到期时间，不关闭告警也不延长截止；恢复后再出现属于新周期。告警台只显示安全分类（kind/source/scope/计数）。仪表盘展示进程与 Worker 状态、邮箱轮询、realtime 滞后、凭据健康、投递/履约/恢复积压（区分等待付款与等待前置）、告警状态与资产对账。资产对账报告缺失时显示不可用，不用空数据冒充无差异；外部账号仅在 Worker control 快照整体不可用时标为不可用。

### 日志

- 所有进程向 stdout 输出 JSON 日志（无 ANSI），由 journald 收集：`journalctl -u auction-worker -o cat | jq`。
- 过滤器由 `common.toml` 的 `log_filter` 控制（默认 `info,<服务名>=debug`）：逗号分隔的 `级别` 或 `target=级别`，只按 target 前缀与级别过滤，语法见[ops/config/README.md#commontoml](config/README.md#commontoml)。
- 日志只包含安全分类与标识（命令 ID、拍卖/lot/义务 ID、错误码），不含连接串、Cookie、密钥、登录 key 或邮件正文。
- 可选 OTLP/HTTP tracing：在角色文件 `[tracing]` 启用，导出头只能写在敏感文件的 `*_otlp_headers` 中（进程环境中的 `OTEL_EXPORTER_OTLP_*HEADERS` 会导致拒绝启动）。

### 安全错误与未持久化指标

日志 target 为 `auction::diagnostics`：`kind` 是简短 owner，`phase` 是语义步骤，`safe_kind` 是安全错误类别；同一二元组只表示同一种原因，禁止序号和模块路径。不同原因不修改持久化 defer reason/失败码，旧版本仍能读取与恢复原事实。owner 清单（按范围）：

| 范围 | 日志 kind |
| --- | --- |
| 管理用例与 Admin 接口 | `admin_intake`、`admin_runtime_config`、`admin_users`、`admin_worlds`、`admin_auctions`、`admin_audit`、`admin_credentials`、`admin_dashboard`、`admin_delivery`、`admin_forum`、`admin_lifecycle`、`admin_lots`、`admin_mail`、`admin_proxy_events`、`admin_realtime`、`admin_recovery`、`admin_reports`、`admin_rules`、`admin_runtime`、`admin_runtime_switches`、`admin_session`、`admin_settlement` |
| 竞价生命周期、慢代理与账本 | `auction_lifecycle`、`slow_proxy`、`competition_ledger`、`competition_history`、`competition_timestamp`、`auction_schedule`、`auction_wakeup` |
| 结算、履约与资金 | `settlement`、`settlement_ledger`、`settlement_fulfillment`、`settlement_delivery`、`fulfillment`、`fulfillment_authorization`、`fulfillment_recipient`、`fulfillment_internal`、`fulfillment_refund`、`funding`、`funding_record_only` |
| 投递与恢复 | `delivery`、`delivery_wakeup`、`recovery_dispatch`、`recovery_resolution`、`recovery_activation`、`recovery_blacklist`、`recovery_health`、`recovery_observation` |
| 邮件与装备 | `mail_intake`、`mail_poll`、`mail_maintenance`、`mail_observation`、`mooglemail_baseline`、`mooglemail_credentials`、`mooglemail_diagnostics`、`mooglemail_evidence`、`mooglemail_identity`、`mooglemail_inventory`、`mooglemail_maintenance`、`mooglemail_read`、`mooglemail_return`、`mooglemail_send`、`mooglemail_session`、`mooglemail_take`、`mail_delivery`、`equipment_crypto`、`equipment_diagnostics`、`equipment_fetch`、`equipment_link` |
| 论坛 | `forum_sync`、`forum_reply`、`forum_notification`、`forum_pm`、`forum_credentials`、`forum_forms`、`forum_profile`、`forum_session`、`forum_topic` |
| 关注、会话与补偿 | `lot_watch`、`lot_watch_preference`、`lot_watch_mail`、`lot_watch_pm`、`public_blacklist`、`session_preference`、`compensation_round`、`compensation_lot`、`compensation_movement` |
| Public 接口与实时传输 | `public_account`、`public_bid`、`public_catalog`、`public_compensation`、`public_competition`、`public_countdown`、`public_history`、`public_proxy`、`public_realtime`、`public_rules`、`public_session`、`public_worlds`、`http_boundary`、`http_listener`、`http_readiness`、`http_response`、`http_transport`、`realtime_hub` |
| 凭据、备份与运行时 | `credential_check`、`credential_crypto`、`credential_login`、`login_key`、`backup_archive`、`backup_heartbeat`、`runtime_switches`、`control_listener`、`control_server`、`control_trigger`、`worker_runtime`、`operations_alert` |

HTTP 用 `.fields.route`（路由模板）与 `.span`/`.spans` 的 request_id 过滤，不用原始路径或请求头。Sent 回退看 `mooglemail_baseline` 的 `fallback_before_send`/`fallback_after_send`；HTTP 就绪看 `http_readiness`；Storage 审计分类仍用 `auction::storage::audit_health`。去重汇总位于 `auction_observability::log_dedup`（固定 kind/phase/safe_kind 与 suppressed_count，汇总不保存业务标识）。不要打开原错误正文、Cookie 或 URL 日志；配置回退沿用 `log_filter`。

无对应持久处置的失败计 `auction_worker_unpersisted_error_total{loop,kind}`：`sum by (loop, kind) (increase(auction_worker_unpersisted_error_total[15m]))`。它与轮次 error、持久重试计数分别解释，不能相加推断失败义务数；提交未知计入但不授权重发。口径见[crates/auction-observability/README.md#未持久化错误](../crates/auction-observability/README.md#未持久化错误)。

### D1 诊断

COD 实收资金化诊断使用独立 target `auction::settlement::funding_diagnostics`。默认 `info,<服务名>=debug` 已包含其 WARN；自定义过滤器屏蔽该 target 时，在 `log_filter` 末尾追加 `,auction::settlement::funding_diagnostics=warn`（关闭用 `=off`），修改后按停机门禁重启。先按 target 收窄，只输出义务/投递/命令标识、`stage`、`safe_kind`、`cause_kind`、`defer_reason` 与 `outcome`：`jq` 模板见[crates/auction-observability/README.md#未持久化错误](../crates/auction-observability/README.md#未持久化错误)。辅助只读诊断只在原授权查询无行时同事务执行，250ms 超时或任一失败归回原泛化拒绝；分类字典与辅助 SQL 语义见[crates/auction-application/README.md#错误诊断](../crates/auction-application/README.md#错误诊断)与[crates/auction-storage/README.md#授权查询无行的只读诊断](../crates/auction-storage/README.md#授权查询无行的只读诊断)。

线上证据核对只选必要标识、状态、时间与布尔比较结果，区分重放、业务拒绝、已确认回滚、未确认结果与 defer 持久化失败；不导出 UID、金额、正文、payload、metadata 或摘要。确认根因后另行批准修复，不据诊断修改人工 Sent、更正原失败命令或自动重发。

## 数据库连接预算与启用顺序

先用 `auction-cli config check --strict` 查看 `database_budget`，再决定是否启用服务超时。默认三服务池各 16（含长期 owner），Public 普通余量 15、Admin 16、Worker 最坏 13；维护池默认 4，预算允许两个 CLI，另预留 8 个专用/外部会话，计划合计 48 + 2 × 4 + 8 = 64。声明 `max_connections=100`、superuser reserve 3 时普通上限 97，未分配余量 33。并行 CLI 是操作约束声明而非跨进程信号量；备份池与 `pg_dump`、重建 guard 与迁移池可能重叠；SQLx 借出并发与物理池容量不同。增加实例、维护命令或 exporter 前须重新核对。字段见[数据库连接分档](config/README.md#数据库连接分档)。

启用顺序：默认 profiled、服务超时关闭，先完成指标采集并观察完整场次；按决策 72 观察 Final 与结算；用户审阅后显式把 readiness 切为 classified（默认 shadow）；用户填写 acquire 2000ms、statement 5000ms、lock 1000ms 后启用超时（idle 默认 0）；经[停机时段保护](#停机时段保护)重启使全部池和长期会话一致。回退先关超时并重启，再考虑 readiness 回 shadow/legacy；`legacy` 为池 16、acquire 30 秒的完整旧路径。重启同样受停机门禁约束。

## 告警处置手册

每个来源对应一行；告警正文只含安全分类与计数。<a id="alert-runbook-process"></a><a id="alert-runbook-delivery"></a><a id="alert-runbook-fulfillment"></a><a id="alert-runbook-recovery"></a><a id="alert-runbook-mail"></a><a id="alert-runbook-credential"></a><a id="alert-runbook-lifecycle"></a><a id="alert-runbook-forum"></a><a id="alert-runbook-notification"></a><a id="alert-runbook-backup"></a><a id="alert-runbook-wal"></a><a id="alert-runbook-reconciliation"></a>

| 来源 | 现象与处置 | 恢复判定 |
| --- | --- | --- |
| process | `service_missing`/`service_degraded`、`audit_failure` 增长：`systemctl status auction-test-{public-api,admin-api,worker}`，核对 `/health/ready` 与 `auction::*` 日志；Worker 反复退出先停 unit，按 [Panic](#panic-排障) 与[告警处置手册](#告警处置手册)的 recovery 行定位。三角色同时降级多为数据库问题：查 PostgreSQL、`statement_timeout` 与[连接预算](#数据库连接预算与启用顺序)。 | 连续两轮均 `ready`，`audit_failure` 不再增长 |
| delivery | `delivery_pending`、`delivery_manual_reconcile`、`delivery_returned_mail`、`queue_latency`：在 Admin“投递队列”按状态过滤，人工对账/退信按[日常运维](#日常运维)处理，绝不自动重发；`queue_latency` 检查 Worker、账号忙与 `request_timeout_seconds` 是否超过停机预算。 | 积压与最老年龄低于阈值，已领取行终态 |
| fulfillment | `fulfillment_backlog`、`fulfillment_failed`、`fulfillment_manual_reconcile`：在结算报表看义务状态，失败项核对 `last_error_code`，人工对账与投递同源。 | 义务 `satisfied` 或进入人工决定，年龄回落 |
| recovery | `recovery_payment_overdue`、`recovery_payment_mail_unsent`、`recovery_relist_pending`、`recovery_failed`、`recovery_manual_reconcile`、`balance_low`、`evidence_stale`、`account_busy`：付款邮件未证明时先核对投递与 Sent，不能重发或改期限；账号忙停止人为触发并等待退避，超过 15 分钟查第二个 Worker/CLI；余额与 evidence 分别补 Credits 或恢复页面读取。 | 两世界余额与 evidence 为 `ok`，逾期/重挂归零 |
| mail | `mail_poll_stalled`、`mail_sent_page_limit_approaching`：核对 `mail_poll_state` 的 `last_poll_at`、`consecutive_failures`、`paused`；页数告警在硬上限 100 内提高 `maximum_pages` 或清理旧 Sent，不能把前缀扫描当完整枚举；Cookie 失效按 credential 处理。 | 成功轮询后 `last_poll_at` 前进、失败数归零 |
| credential | `credential_invalid`（`mail_cookie`/`external_api`）：Admin“Cookie 池”录入新值并停用旧值（每种 kind 只能一个 active）或 `credential import`，再 `service restart`；疑似账号被盗按[游戏页面变化与账号风险](#游戏页面结构变化与账号风险)处理。 | 当前行不再是 `invalid`，收件与论坛写入恢复 |
| lifecycle | `auction_phase_overdue`、`proxy_expiry_overdue`、`season_obligations_pending`：确认 Worker 生命周期循环运行且未被结算长期占用；前两者由 Worker 推进，换季义务到边界后由提醒与人工收口；Final 期间故障只告警、不延长截止。 | 场次进入 Final/结算、到期代理完成，计数归零 |
| forum | `forum_sync_failed`、`forum_message_failed`、`forum_topic_page_limit_approaching`：`auction-cli forum recovery` 查看写入未知/失败主题，`pm list`/`pm reconcile` 处理私信（进入人工对账说明可能已发出）；页数告警确认硬上限与主题拆分需求。 | 主题 `published`、私信 `succeeded` 或人工对账完成 |
| notification | `notification_delivery_failed`、`notification_postage_insufficient`：邮资不足时补充 Bot Credits 后在“投递队列”重试，补足前不要反复重试。 | 失败与最老年龄归零，新通知可寄出 |
| backup | `disk_capacity_low`、`backup_failed`、`restore_drill_overdue`、`source_unavailable`：`df -h /var/backups/auction`；终态与失败阶段见 `<state-dir>/receipts/<run_id>.json`，底层错误见 `journalctl -u auction-backup`；演练逾期按[从 R2 取回并演练恢复](deploy/RECOVERY.md#从-r2-取回并演练恢复)完成并把回执复制到演练目录。 | 一次 `completed` 且空间满足字节与比例下限 |
| wal | `wal_upload_stalled`、`source_unavailable`、`wal_offsite_disabled`：核对 `journalctl -u auction-wal-upload` 与 `wal upload` JSON，检查 R2 凭据、bucket lock 与网络；积压会阻塞 PostgreSQL 回收，优先处理。`wal_offsite_disabled` 只是报告跳过项。 | `wal upload` 为 `completed` 且暂存无超阈段 |
| reconciliation | `custody_drift`、`solvency_shortfall`、`reconciliation_stale`、`source_unavailable`：`db asset-reports` 读取最近报告并按 `missing_samples.reason` 核对 Armory 与 delivery；`db report-solvency` 重算并核对余额状态。报告只读，不授权转账、退款、重发或转移；Armory 只扫描七个 organize 分类，未命中不代表装备不存在。 | 下次报告无差异且年龄在阈值内 |

### 游戏页面结构变化与账号风险

- 页面结构变化（装备分组、Sent 列表、固定楼层标记）：表现为 `mail_poll_stalled`、协议漂移的投递失败或论坛同步失败。先保存脱敏页面样本，再按对应来源处置；不要放宽解析或改用人工确认绕过证据。
- 账号被盗或被封：立即停止 Worker，停用该 Cookie 与相关 key，替换为新账号凭据；同时核对 Sent 与论坛是否出现未知写入，结果未知的一律转人工对账。
- 恢复判定：结构与登录恢复正常，且不再出现新的协议漂移失败。

## 日常运维

| 任务 | 做法 |
| --- | --- |
| 更换外部 Cookie | Admin “Cookie 池”录入新凭据并停用旧凭据（每种 kind 同时只能一个 active），或 `auction-cli credential import`；然后重启 Worker（分体部署用 `service restart`） |
| 暂停/恢复收件 | Admin 仪表盘或“收件审核”页；恢复轮询不绕过场次冻结的赛季收件门禁，见[docs/business-rules.md#赛季早鸟期auctionscheduleearly_bird](../docs/business-rules.md#赛季早鸟期auctionscheduleearly_bird) |
| 结果未知的投递 | Admin “投递队列”按动作与证据人工对账；主动 Send 禁止仅凭外部引用人工确认成功，须由冻结的精确 Sent 证明恢复。未找到 Sent 不证明未发送 |
| 恢复确认 | Admin “结算报表”中确认 `reclaimed` / `cancelled` / `equipment_recovered`；付款截止与跨赛季重列限制见[4 恢复付款超时与拒收](../docs/guide/settlement.md#4-恢复付款超时与拒收) |
| 论坛写入未知与私信对账 | `auction-cli forum recovery` 查看，必要时 `forum authorize-retry`；私信用 `pm list` / `pm reconcile` |
| 规则与“关于”发布 | Admin “规则内容”“关于内容”页（内容直接写入数据库，没有文件导入） |
| 查看部署历史 | `auction-cli service receipt list` / `service receipt show <receipt_id>`；回执由 `service start/restart` 追加，含程序摘要、迁移版本、恢复点、操作者与时间 |
| 修改运行参数 | Admin “参数配置”页（校验后原子写入 runtime 文件），随后重启相关角色生效 |
| Bot 资金账户 | `auction-cli funds preview` 可在线只读执行；开户或现金对账写入前须先停止 Worker，见[docs/guide/settlement.md#bot-内部履约与资金账户](../docs/guide/settlement.md#bot-内部履约与资金账户) |
| 导出邮件诊断证据 | `auction-cli mail export-evidence <evidence-id> --output <文件>` |
| 数据保留与清理 | `auction-cli db inventory` 只统计不授权删除；`db retention-dry-run` 按政策 v2 写 run id 回执；执行用 `db retention-purge --dry-run-receipt <回执> --approve <family>=<token> --execute --confirm 'PURGE <库名>' --receipt <回执>`，只删运行态族、逐族令牌绑定演练计数并追加 `auction.retention.purge` 审计，审计/资金/业务事实与有效会话永不删除 |

## 备份与恢复

例行备份用 `auction-cli backup scheduled` 配合独立 systemd timer，包含磁盘门禁、本地保留与不可变回执；可选 `[backup.scope]` 把运行配置、加密诊断证据与部署回执打包为 age 密文，并用 `backup scope-extract` 校验。启用与故障处理见[定时备份](deploy/RECOVERY.md#定时备份)；升级前恢复点继续用手动 `backup create/verify`；恢复、重建与演练见[RECOVERY](deploy/RECOVERY.md)。

WAL：`archive_command` 调用 `wal archive` 压缩、age 加密并 fsync 到本机暂存；`auction-base-backup.timer` 每周运行 `wal base-backup`，`auction-wal-upload.timer` 每 5 分钟运行 `wal upload` 上传 R2 的 `<prefix>/wal/` 与 `<prefix>/base/`，校验后删本地 WAL、基础备份保留最近 3 份。`archive_mode` 默认 `off`，开启须重启 PostgreSQL 并受[停机时段保护](#停机时段保护)约束；顺序与回退见[WAL 归档与时间点恢复](deploy/RECOVERY.md#wal-归档与时间点恢复)。

## 故障排查

| 现象 | 排查 |
| --- | --- |
| 进程不就绪，日志提示 schema 版本不符 | 停服后执行 `auction-cli db migrate --owner-secrets <owner 文件>`（分体部署用 `service restart --owner-secrets <owner 文件>`） |
| 预检报“服务数据库账号 … 不是迁移 081 授权的运行账号” | 三角色 `secrets.toml` 的 `[database].username` 改为 `auction_runtime`，库 owner 保持迁移 owner；`common.toml` 的 `[database_roles] owner` 填迁移 owner 名 |
| 迁移报“运行时角色 auction_runtime 不存在” | 先按首次部署步骤创建该 LOGIN 角色，再重跑迁移；不要用 owner 账号顶替服务账号 |
| `service start` 卡在 Worker readiness | 确认 `[worker.control]` 已启用、`allowed_peer_uids` 含 0、`/run/hv-auction/worker` 存在且权限正确 |
| Admin 启动报 runtime 文件与启动配置不一致 | runtime 文件已被更新但进程尚未用新配置重启：用 `auction-cli service restart` 整体重启 |
| Worker 启动失败：凭据所有权已被占用 | 另一个 Worker（或正在执行的 `auction-cli funds`）持有邮件/论坛 Cookie 所有权；确认只运行一个 Worker |
| Public 启动失败：realtime consumer 已被占用 | 两个 Public 实例使用了相同的 `realtime.consumer_key` |
| 浏览器写请求返回 `origin_forbidden` | `allowed_origins` 与浏览器访问的 origin 不完全一致（协议、主机、端口） |
| 所有客户端被同一地址限流 | 反向代理地址未加入 `trusted_proxy_cidrs`，服务端把代理当作客户端 |
| 页面返回 503 | 程序不是 `web-embed` 构建，或内嵌资源缺失 |
| 投递长时间停在 `manual_reconcile` | 结果未知，需在“投递队列”人工对账；不要直接改数据库。对账返回 `delivery_sent_sighted` 时说明 Sent 已有该结算义务的邮件，改用 `auction-cli settlement confirm-sent` |
| `deploy` 报“仍有 N 笔已越过 sending 围栏的投递在途” | 门禁只拒绝部署，不改队列或租约。等待这些 delivery/PM 完成（或在 Admin 核对后处置），必要时调大 `--send-gate-timeout-seconds`；不得用停机打断外部写入 |
| `deploy` 报“制品内嵌 schema 版本低于数据库” | 这是回退方向：不要用 `deploy` 覆盖新版库；按[程序回退](deploy/RECOVERY.md#程序回退)核对 `(E,D]` 区间，含 `breaking` 时恢复升级前备份 |
| `deploy` 报“迁移演练回执核对失败” | 逐项核对回执：7 天内、提交/包名/CLI 摘要与本次制品一致、备份 schema 等于当前库、迁移区间恰好为当前→候选、readiness 全 200 且 `consistency_check.work_item=W11.4`、`conclusion=passed`、无 `anomalies`、窗口不低于三倍实测；不符就重做演练，不改回执 |
| `service restart` 报“数据库连接预算”相关错误 | 按[数据库连接预算与启用顺序](#数据库连接预算与启用顺序)核对三角色 `[database_pool]`、并行 CLI 与外部预留，不要只调大单角色池 |


### Panic 排障

release 开启整数溢出检查，原生保留 `panic = "unwind"`；溢出会失败关闭并进入已有 panic 边界，但**不证明业务已回滚**，尤其不能据此重发外部写入。WASM 继承开启检查并保留 abort，现有 hook 展示静态错误横幅。先按 `auction::panic` target 定位线程与源码 `file`、`line`、`column`；`payload` 只显示静态字符串，格式化 String 或其他类型均为固定占位，顶层 join 日志只显示 `panic`、`cancelled` 或 `other`。不要求采集完整 payload、凭据或外部响应来排障。再查看 `auction_worker_task_panics_total{loop,owner}` 的增量，关联 `auction::diagnostics` 的 `phase=item_panic`、`safe_kind=task_panic`；该指标按闭集循环预置零值，进程重启归零，只覆盖工作项子任务的 join 收口，不是工作项尝试次数。

| 工作项 | panic 后的现状 |
| --- | --- |
| 生命周期 | 首次 panic 写 fenced terminal，后续自动领取排除；人工按 owner 路径处理 |
| 结算 | 首次 panic 写 terminal 并追加审计，后续自动领取排除 |
| 履约、恢复 action | 首次 panic 转 `manual_reconcile`，不自动重试 |
| delivery | claimed 可重领并计次；sending 到期加 grace 后转人工 |
| 邮箱轮询 | panic 不增加连续失败数；重启后可重领，成功清零 |
| 论坛 | 发布领取计次但无封顶；过期 claim 按未知写入对账 |
| PM | 发送前可重领；sending 到期转人工，只读对账不重寄 |
| 告警批次 | 已有三次上限，耗尽进入 failed/retry_exhausted |

慢速代理与直接关键循环仍失败关闭；邮件、论坛、PM、告警叶子直接执行，panic 会退出常驻任务并触发 Worker 排空并非零退出（分体 Worker 由 `Restart=on-failure` 拉起，Public/Admin 不连带停止）。普通 Public/Admin handler panic 由 HTTP 边界捕获并返回 500，不等同于整个角色退出。系统不提供新的通用人工恢复 CLI：先核对持久状态和原命令结果，再决定人工操作；未知外写绝不自动重发。

### SQL 失败诊断

先按既有命令、拍卖、lot 或投递标识关联失败日志。带安全上下文的错误在原中文原因后附加方括号字段：`sqlx_kind` 区分数据库错误与连接池、传输、解码等失败；`sqlstate_class` 为两位类别，`sqlstate` 为五位代码；`constraint`、`table` 可定位迁移中的约束、索引或表。字段规则的唯一说明见 [crates/auction-storage/README.md#sql-错误安全上下文](../crates/auction-storage/README.md#sql-错误安全上下文)。

`none` 表示驱动未提供该字段，`invalid` 表示诊断字段格式异常；部分触发器错误没有约束名或表名，不能据此认定没有约束拒绝。结合制品/迁移版本、原业务上下文和持久状态排查，不收集驱动 message/detail/hint、SQL、绑定值、连接串或凭据。仅记录 `safe_kind` 的日志仍只有分类，并非每个失败入口都输出增强文本。SQLSTATE 和错误文本用于定位原因，不能据此自动重试、重发邮件或认定提交已回滚；继续遵循原事务 owner 的类型化判定与未知结果对账流程。本项不新增配置，回退为换回旧制品；日志过滤器不能撤销已经写入日志或失败安全详情的增强文本。

## 运行时开关

已开放 `readiness_audit_failure_mode`（三角色）、`readiness_cache_enabled`（Public/Admin）、`metrics_sli_snapshot_enabled`（Worker，基线 false）、`mooglemail_poll_isolation_enabled`（Worker，基线 true）、`metrics_reject_forwarded_requests`（三角色，基线 true）、`external_access_paused`（三角色，无 TOML 基线）与 `disposition_read_mode`（Admin/Worker，无 TOML 基线）。通过 `auction-cli switch list` 读取 revision，再用稳定命令 ID、期望版本、actor 与原因 set/clear，参数见[运行时开关](../apps/auction-cli/README.md#运行时开关)。切 classified 前须审阅[影子样本](#审计就绪影子观察)，系统不自动推进。

`disposition_read_mode` 缺省 `shadow` 继续显示旧口径裁决值，只把分类差异写进 `auction::shadow` 与 `auction_shadow_mismatch_total{owner="disposition_reads"}`；`classified` 才把统一处置事实变成可见口径（须先观察差异并经用户批准）；`legacy` 是回退，只改读谓词、不删处置事实。两条口径由同一语句级快照配对读取。`metrics_reject_forwarded_requests` 是安全策略，只允许在线收紧：`set true` 和 `clear` 成功，`set false` 固定拒绝并以退出码 5 失败；放宽须改 common 的 `[metrics] reject_forwarded_requests` 并重启。

set 以未过期数据库事实覆盖启动时 TOML；clear 或到期只回到基线（不一定是 legacy/false）。无期限覆盖需 `--no-expiry`，跨重启保留；改 TOML 不会清除它。三角色启动即读，此后五秒刷新、每次一秒预算；首次失败用基线，之后失败保留最后成功快照，原期限仍生效。Admin `/config` 卡片显示数据库意图、版本、期限、操作者、原因与 Admin 自身实际值，不能替 Public/Worker 确认。通过各角色受限指标入口核对 `auction_runtime_switch_refresh_successes_total`、`..._failures_total`、`..._snapshot_age_seconds`、`..._applied_revision` 与 `..._effective_value`；年龄 -1 表示未成功，明显超过两个周期按 stale 处理。审计分类累计不因切换清零；程序回退须使用已登记兼容且显式启用兼容模式的旧程序，按[程序回退](deploy/RECOVERY.md#程序回退)核对超前区间。

<a id="主动暂停外部访问w19"></a>

#### 主动暂停外部访问

人工使用账号、事故处置或恢复后安全启动时，用带到期时间的暂停标记阻止**全部**新外部请求。可在 Admin `/config` 的运行时开关卡片中两阶段确认后暂停或恢复（期限最长 7 天，结果未知时按“原样重试”提交同一命令），也可用 CLI：

```bash
auction-cli --config-dir /etc/auction switch set external_access_paused true \
  --expires-at 2026-11-04T12:00:00Z \
  --command-id <稳定UUID> --expected-revision <list中的版本> \
  --actor <操作者说明> --reason <非接触发账号的操作>
```

- Worker 在每个循环边界判定：MoogleMail 收件与维护、delivery 发送、结算履约、恢复、资产对账、论坛同步与回帖导入，以及被超提醒的论坛私信（发送与只读核对）都不开始新轮次或新批次；Admin 手工装备读取与 CLI `mail inspect-sent` 因经 Worker control 执行而同样被拦截（`409 paused` / CLI 明确报“外部访问已暂停”）。
- 与按世界暂停收件并存：按世界暂停只停该世界的业务收件，全局暂停停全部外部访问；两者同时生效时以全局暂停为准。
- 暂停只阻止发起新请求，不撤销已进入 sending 围栏或已领取的写入；这些写入按自身 lease 与预算完成，不会重发。
- 到期由数据库事实自动回落（`expire` 收口由 Worker 追加），不需要手工 clear；需要立即恢复时执行 `switch clear external_access_paused`。核对方式与其它开关相同：Admin `/config` 卡片 + `auction_runtime_switch_effective_value{switch="external_access_paused"}`。

