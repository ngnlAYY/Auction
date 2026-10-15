# systemd 服务与顺序控制

[文档导航](../../../docs/README.md) · [部署](../README.md) · [总控 unit](auction-stack.service.example) · [Public](auction-public-api.service.example) · [Admin](auction-admin-api.service.example) · [Worker](auction-worker.service.example)

分体部署由 systemd 管理单角色 PID、信号、重启和日志，由 auctionctl 管理跨角色顺序、readiness、锁与回执。
合并部署只启用 [auction-server.service](auction-server.service.example)，不得同时启用分体 unit/总控；
首次向导与 migrate-managed 见[server 手册](../../../apps/server/README.md)。

## 安装前提

所有 `.example` 都是待审阅模板，不自动安装。先替换 User/Group、程序和配置路径，准备运行目录与用户组，
核对敏感 TOML 可读性、Worker control allowlist、Public/Admin listener 和代理上游。
首次配置和数据库前置条件见[部署手册](../README.md#首次部署配置)。

## 安装示例

审阅并完成上述准备后安装四份分体 unit：

```bash
sudo cp ops/deploy/systemd/auction-public-api.service.example \
  /etc/systemd/system/auction-public-api.service
sudo cp ops/deploy/systemd/auction-admin-api.service.example \
  /etc/systemd/system/auction-admin-api.service
sudo cp ops/deploy/systemd/auction-worker.service.example \
  /etc/systemd/system/auction-worker.service
sudo cp ops/deploy/systemd/auction-stack.service.example \
  /etc/systemd/system/auction-stack.service
sudo systemctl daemon-reload
sudo systemctl enable --now auction-stack.service
```

## 启动和停止顺序

只 enable 总控：严格预检同一公共配置、权限、unit 环境及监听占用，三角色明确停止后才迁移，
再按 Worker → Admin → Public 逐个 readiness；停止反序，失败仅停止本次启动的 unit，不回滚已提交迁移。
完整 start/restart 分支见[CLI 总控](../../../apps/auctionctl/README.md#systemd-总控)。

Public Requires Admin/Worker，Admin Requires Worker；单角色停止/重启可能联动依赖者，操作前须核对实际 unit，不能按命令点名范围推断影响。

## 配置绑定与探针

AUCTION_CONFIG_FILE 仅接受 Environment 中无空白/转义的简单赋值，可整体加双引号。
拒绝非空 EnvironmentFiles、UnsetEnvironment、PassEnvironment、PAMName，不回显环境值。
systemctl show 仅可省略空 EnvironmentFiles，其他身份/环境字段必须返回。

Worker control 须启用并允许实际 Admin/运维 UID 或 GID，探针走 Unix `/v1/health/ready`；
Public/Admin 走有效 TCP/Unix `/health/ready`，Admin 不回退 runtime 覆盖前地址。
每次异步连接、写入、状态行读取共用最多 1 秒且受剩余启动预算约束；每角色默认 30 秒。

## 关闭预算

| 边界 | 当前预算 | 安装时核对 |
| --- | --- | --- |
| 单角色 | drain 10 秒 + exporter 最多 30 秒；unit 45 秒 | 二次信号强制结束 drain，exporter 仍受自身超时保护 |
| 分体总控 | TimeoutStartSec=180、TimeoutStopSec=120 秒 | 启动须覆盖迁移和三个 readiness；若经总控 unit 停止且三角色各用满 45 秒，合计 135 秒超过外层预算，安装时须重算 |
| 合并 supervisor | drain 135 秒，强杀后每角色回收最多 5 秒；unit 155 秒 | KillMode=mixed，超时回收整个 cgroup；不得套用分体 45 秒 |

这里只核对模板和源码预算，不代表最长关闭路径已经运行验证。改变任何阶段上限时须同步重算外层超时。

## 权限与回执

StateDirectory=auction/deployment 创建私有状态目录；总控持非阻塞锁、原子发布 startup-order.v1.json。
手工运行须预建规范绝对路径，归有效用户所有、权限至多 0700。
回执仅含 unit、InvocationID、PID、单调启动时间及观察时间，不含配置/凭据。
全部已运行时 start 复核 readiness/顺序并发布 adopted 回执；部分运行或转换中拒绝。

须预建 auction-worker-control 组，并纳入 Worker/Admin supplementary group；TOML group、allowlist 须匹配实际 UID/GID，禁止照抄数字 ID。
Admin/Worker 须匹配 control 路径、三组数据密钥及有效拍卖/结算/排期；operations 接管须共用 runtime。
保存不热加载，重启范围见[配置说明](../../config/README.md#runtime-共享与重启)。

## 状态与重启

```bash
sudo /opt/auction/bin/auctionctl deployment status
sudo /opt/auction/bin/auctionctl deployment restart --config-dir /etc/auction
```

进程存活不能代替 readiness；数据库不可用或 schema 未就绪时 ready 保持 503。
重启 Admin 会撤销旧管理会话，操作者须重新登录。

## 可写目录

Admin 模板开放 `/etc/auction/runtime` 和 `/run/hv-auction`；Public 开放后者。
Worker 用 RuntimeDirectory 预建 `/run/hv-auction/worker`，开放 `/run/hv-auction`、`/var/lib/auction`。
按实际 socket 和诊断目录收窄或补齐 ReadWritePaths，公共/敏感 TOML 保持只读。
目录 owner/group、实例锁、inode 和 peer allowlist 仍由应用检查，systemd 可写声明不能替代这些门禁。

## 独立迁移门禁

`auctionctl migrate` 沿用总控的 systemctl、state-dir 和三个 unit 参数，持锁要求全部角色 inactive/failed 且 MainPID=0；
运行、转换中、状态读取失败或锁冲突均阻断迁移。总控 start/restart 在同一锁/预检边界迁移，单角色 unit 不迁移。
参数及建库边界见[CLI](../../../apps/auctionctl/README.md#停机迁移与初始建库)；任何 unit 均不执行 DROP/CREATE、备份恢复或 retention 删除。
