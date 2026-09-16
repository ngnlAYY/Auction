# systemd 服务与顺序控制

[文档导航](../../../docs/README.md) · [总控 unit](auction-stack.service.example) · [Public unit](auction-public-api.service.example) · [Admin unit](auction-admin-api.service.example) · [Worker unit](auction-worker.service.example) · [控制 owner](../../../apps/auctionctl/src/bootstrap/deployment_control)

模板需替换账号、路径和权限；单进程生命周期属于 systemd，跨角色顺序属于 auctionctl。

## 本页导航

- [安装前提](#安装前提)
- [安装示例](#安装示例)
- [启动和停止顺序](#启动和停止顺序)
- [配置绑定与探针](#配置绑定与探针)
- [关闭预算](#关闭预算)
- [权限与回执](#权限与回执)
- [SQLite 文件权限](#sqlite-文件权限)
- [状态与重启](#状态与重启)
- [可写目录](#可写目录)
- [独立迁移门禁](#独立迁移门禁)

## 安装前提

这些 unit 是当前三进程边界的审阅模板，文件名带 `.example`，不会被自动安装。安装前必须
由部署者把 `User`、`Group`、二进制路径、配置路径和目录权限替换为实际值，并确认运行账号
能读取对应敏感 TOML。`auction-stack.service` 是可选的宿主机总控 unit；它调用 Rust
`auctionctl deployment`，单个进程的 PID、信号、停止超时和日志仍由 systemd 拥有。


## 安装示例

```bash
# 复制并审阅四个示例后再安装
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

不要同时 enable 三个角色 unit；否则系统启动会绕过总控的应用 readiness 顺序。总控会先严格
检查三个角色 TOML 对同一公共 TOML 的引用、各文件权限、unit 的 `AUCTION_CONFIG_FILE` 绑定和端口占用，再执行 forward-only
migration，并按 Worker → Admin → Public 启动。每个角色通过应用 readiness 后才继续；失败只反序
停止本次启动的 unit，已提交 migration 不会自动 down/restore。停止顺序固定为
Public → Admin → Worker。


## 配置绑定与探针

角色 unit 的配置绑定只支持 `Environment` 中无空白、无转义的简单赋值；可整体包裹双引号。
总控会拒绝非空 `EnvironmentFiles`、`UnsetEnvironment`、`PassEnvironment` 和 `PAMName`，
避免间接覆盖或删除 `AUCTION_CONFIG_FILE`，也不会把环境原值写进诊断。`systemctl show` 会省略
空的 `EnvironmentFiles` 复合数组，因此仅该属性允许缺行表示空；其余身份和环境属性仍必须返回。
readiness 的 TCP/Unix
连接均为非阻塞异步操作，连接和 HTTP 写入/读取共用单次 1 秒上限，并受角色启动剩余预算约束。


## 关闭预算

三个角色 unit 都以 SIGTERM 进入同一关闭路径：先进行最多 10 秒的 HTTP/Worker drain，再按配置
刷新 tracing exporter（最大 30 秒）。示例固定 `TimeoutStopSec=45s`，覆盖两段上限和 5 秒回收
余量；排空期间第二次 SIGINT/SIGTERM 会强制结束业务 drain。若部署方修改 tracing 上限或进程
关闭阶段，必须同步重新核算 unit 超时，避免 systemd 在 exporter 有界关闭前发送 SIGKILL。


## 权限与回执

`auction-stack.service` 通过 `StateDirectory=auction/deployment` 建立私有控制目录，并保存互斥锁与
`startup-order.v1.json`。回执只包含 unit、systemd `InvocationID`、PID、单调启动时间和观察时间，
不包含配置正文或凭据。Worker TOML 必须启用 `[worker.control]`，并把实际 Admin/运维 UID 或 GID
写入 allowlist，否则总控无法证明 Worker readiness。示例 unit 假定已建立
`auction-worker-control` 组，并把 Worker 与 Admin 都加入该 supplementary group；部署者若使用
不同账号或组名，必须同步修改 unit 和 TOML，不能照抄主机相关数字 ID。
严格预检还要求 Admin 与 Worker 的 control socket 及三组数据密钥完全一致；Admin runtime 若
选择 Unix listener，总控会直接通过该 socket 检查 `/health/ready`，不会退回主配置 TCP 地址。
接管 runtime 的任一 `operations` 组时，Admin/Worker 还必须引用同一份 runtime 文件，且两者的
有效拍卖默认模板与周排期必须一致。经济模板/排期更新后重启这两个角色，其余运行参数组重启
Worker；unit 不会把 runtime 文件保存操作解释为热加载。


## SQLite 文件权限

该总控模板以 system manager 权限运行，默认面向 PostgreSQL 生产档。SQLite Lite 若使用同一模板，
必须先由部署者保证数据库文件及父目录归 `auction` 运行账号所有；不要让首次 root migration 创建
只有 root 可访问的 SQLite 文件。也可以把总控改为经授权的同一运行账号，但必须同时审阅其
systemd/Polkit 控制权限。


## 状态与重启

启动后仍应从反向代理或本机读取 `/health/live` 和 `/health/ready`。systemd 的进程存活状态不能
替代 readiness；数据库连接失败或迁移未完成时，`/health/ready` 必须保持 503。可用以下命令查看
总控证据或显式重启：

```bash
sudo /opt/auction/bin/auctionctl deployment status
sudo /opt/auction/bin/auctionctl deployment restart --config-dir /etc/auction
```


## 可写目录

Admin runtime 更新需要写入 `/etc/auction/runtime/`，Public/Admin Unix listener 需要写入
`/run/hv-auction`，因此对应 unit 必须显式开放这些目录，并保证运行账号属于配置的 socket group；其余
公共 TOML 和敏感 TOML 仍保持只读。Worker unit 预建 `/run/hv-auction/worker`，Worker 会校验目录 owner、
不可被 other 写入及 group-write 一致性，再以实例锁和 inode 身份保护 control socket；unit 只在
SQLite Lite 部署时开放 `/var/lib/auction` 数据目录。

单个角色 unit 不执行 migration；只有操作员显式调用 `auctionctl deployment start/restart` 或启动
已审阅的总控 unit 时才会前向迁移。不要把 `DROP DATABASE`、备份恢复或 retention 删除动作放进
任何 unit。备份、恢复和保留策略仍需独立的人工确认与证据契约。


## 独立迁移门禁

独立执行 `auctionctl migrate` 时必须使用同一组 `--systemctl`、`--state-dir` 和三角色 unit 参数。
命令会持有总控锁并检查 Worker、Admin、Public；任一 unit 运行或处于启动/停止转换中，或状态读取
失败，都会在连接数据库前失败关闭。总控 `deployment start/restart` 的内部迁移不受该独立 guard
阻断，因为它已经在同一受控锁和配置预检边界内编排迁移。
