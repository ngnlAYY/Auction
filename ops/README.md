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
| `auction-server.service` | 合并部署专用；`KillMode=mixed`、`TimeoutStopSec=155s`、`Restart=no` |

角色 unit 都设 `TimeoutStopSec=45s`（10 秒排空 + tracing 刷新余量）与一组 systemd 加固选项（`ProtectSystem=strict`、`NoNewPrivileges` 等）。

**为什么用 `auction-cli service` 编排**：systemd 的 `After=` 只保证“进程已启动”，不保证“已就绪”。`auction-cli service start` 在每个角色启动后等待其 readiness（Worker 为 control socket 的 `/v1/health/ready`，Admin/Public 为 `/health/ready`，且期间 `MainPID`/`InvocationID` 不变）才启动下一个；单角色 readiness 的完整超时预算覆盖 unit 状态查询、探测、身份复核与轮询等待，`systemctl start` 自身另有 90 秒上限。任何一步失败都会反序停止本次启动的 unit。它还在启动前执行严格配置检查与数据库迁移。详见 [auction-cli](../apps/auction-cli/README.md#分体部署编排)。

常用操作：

```bash
auction-cli --config-dir /etc/auction service status     # 三个 unit 的状态与 readiness（未全部就绪时退出码 4）
auction-cli --config-dir /etc/auction service restart    # 预检新配置 → 反序停止 → 迁移 → 顺序启动
systemctl stop auction-stack.service                     # 按 Public → Admin → Worker 停止
```

注意：

- 三个角色 unit 的 `Requires=`/`After=` 不提供应用 readiness 编排；单独重启 Worker 可能影响依赖角色的运行状态。需要整体重启时使用 `auction-cli service restart`。
- Worker 在凭据所有权失效、control 服务停止等运行时故障时以非零状态退出，由 `Restart=on-failure` 拉起。
- 迁移、建库、重建与恢复要求三个 unit 全部停止（或使用合并部署的 owner 锁），见[停机门禁](../apps/auction-cli/README.md#停机门禁)。

## 升级

分体部署：

1. 阅读版本说明；如有配置变更，先在新配置上执行 `auction-cli config check --strict`。
2. 停服并建立确定的恢复点：`auction-cli service stop`，然后 `auction-cli backup create --database auction --output /var/backups/auction/auction-<日期>.dump`。
3. 保留旧程序以便回退，安装新程序到 `/opt/auction/bin/`。
4. `auction-cli --config-dir /etc/auction service start`：严格预检 → 迁移 → 按序启动并等待就绪。
5. `service status` 确认就绪，检查 Admin 仪表盘。

不需要确定恢复点的小版本（无迁移）也可以直接 `auction-cli service restart`：新配置预检通过后才停止旧服务，然后迁移并按序启动。

合并部署：`systemctl stop auction-server` → 备份 → 安装新程序 → 以服务账号执行 `sudo -u auction auction-cli --config-dir /var/lib/auction/config db migrate` → `systemctl start auction-server`。

迁移 `000069` 后新建场次使用统一加价与代理上限出价规则；升级前已经创建的场次（包括草稿与预览）继续原竞价策略，不自动切换。必须停止全部旧角色、备份、迁移后同版启动；不能让旧 Public/Worker 忽略新快照版本继续写入。场次快照约束见[数据库契约](../docs/contracts/database.md#拍卖与竞价)。

迁移 `000071` 把两个世界的补偿池余额相加，合并为一个以 main Credits 计的全局池，并冻结旧的按世界池表；旧程序写旧表会失败关闭，因此同样必须停止全部旧角色后迁移。合并规则见[结算与履约](../docs/guide/settlement.md#补偿池)。

**回退**：迁移只向前。旧程序遇到更高的 schema 版本不会就绪，所以已迁移后的回退只能是“旧程序 + 恢复升级前的备份”，见[备份与恢复](deploy/RECOVERY.md)。

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
| Public / Admin | `GET /health/live`、`GET /health/ready`（数据库 schema 为当前版本、无审计写入失败；Public 另要求 realtime relay 新鲜） |
| Worker | control socket 的 `GET /v1/health/ready`，例如 `curl --unix-socket /run/hv-auction/worker/control.sock http://localhost/v1/health/ready`（调用者须在 peer allowlist 中） |
| 全部 | `auction-cli service status`（分体部署） |

三个角色每 10 秒向数据库写入心跳（TTL 30 秒）；Admin 仪表盘汇总各进程状态，过期心跳显示为缺失。

### 指标

Public/Worker 在 `common.toml` 设置 `[metrics] enabled = true`；Admin 配置了 runtime 文件时，以 runtime 的 `[metrics]` 为准。Public/Admin 在各自 listener 上提供 `/metrics`（listener 必须是 loopback TCP 或 Unix socket），Worker 在启用的 control socket 上提供 `/metrics`。指标族见 [auction-observability](../crates/auction-observability/README.md#指标)。建议关注：

- Worker 循环迭代与调度延迟、批次饱和、重试次数；
- 数据库连接池等待与空池次数；
- tracing 导出失败或降级；
- 业务事件计数。

### 运维告警

启用 `[worker.operations_alert]`（需要 MoogleMail 已启用且收件人在同一世界）后，Worker 定期扫描并通过 MoogleMail 发送告警邮件：投递积压（数量 / 最老年龄）、需要人工对账的投递、履约积压、付款逾期以及进程健康。同一信号有冷却时间（默认 30 分钟），通知按窗口合并。

### Admin 仪表盘

仪表盘集中展示：进程与 Worker 状态、各邮箱轮询、realtime 滞后与失败、凭据健康、投递/履约/恢复积压与告警状态。各分区独立降级。

### 日志

- 所有进程向 stdout 输出 JSON 日志（无 ANSI），由 journald 收集：`journalctl -u auction-worker -o cat | jq`。
- 过滤器由 `common.toml` 的 `log_filter` 控制（默认 `info,<服务名>=debug`），使用 `EnvFilter` 语法。
- 日志只包含安全分类与标识（命令 ID、拍卖/lot/义务 ID、错误码），不含连接串、Cookie、密钥、登录 key 或邮件正文。
- 可选 OTLP/HTTP tracing：在角色文件 `[tracing]` 启用，导出头只能写在敏感文件的 `*_otlp_headers` 中（进程环境中的 `OTEL_EXPORTER_OTLP_*HEADERS` 会导致拒绝启动）。

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

定期执行 `auction-cli backup create` 并把 archive 与 manifest 复制到异地；恢复、重建与演练流程见[备份与恢复手册](deploy/RECOVERY.md)。

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
