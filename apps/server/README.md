# auction-server

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [部署手册](../../ops/README.md)

Linux x86_64/PostgreSQL 单主机的可选单文件服务：supervisor 用同一可执行文件派生 Worker、Admin、Public，
复用各 app 的库装配，保留独立进程、路由和权限边界。`auctionctl` 仍独立，默认分体发布不变。

装配入口在编译期拒绝其他 OS/架构；浏览器 WASM 保留，默认发布 ABI 见[开发指南](../../docs/development.md#工具与依赖来源)。

## 构建与文件

```bash
just build-single
just release-check --single-binary
```

`build-single` 可在开发工作树构建并核验资源边界；正式 `release-check` 要求干净、已提交源码，均不运行测试。
二进制位于 `target/release-artifacts/single/bin/`，归档位于 `target/release-package-single/`，
可执行文件白名单仅 `auction-server`、`auctionctl`。默认无参数 release-check 仍生成四程序分体包。
`postgres` 启用完整后端，`web-embed` 包含两个独立 Web payload，`cache-redis` 可选。

## 首次启动

先准备 PostgreSQL 18 专用空库和应用账号，以最终服务账号在其拥有、不可被其他账号写入的父目录执行：

```bash
./auction-server --config-dir /var/lib/auction/config
```

`--config-dir` 默认 `auction-config`，相对当前目录解析；设置了 `AUCTION_CONFIG_FILE`（包括空值）会被拒绝。
只有目标目录明确不存在时才开放 `http://127.0.0.1:8091/setup`，终端打印一次性部署码。
网页验证后填写数据库、管理员、精确站点 origin 等参数，明确确认专用空库初始化，再保存并启动。
配置先原子发布，再初始化数据库，最后顺序启动角色；保存响应只证明落盘，以各角色 readiness 判断启动结果。
Public/Admin 端口默认 8090/8091，可选 1–65535 且不同，固定 loopback；须自行同步代理并核对端口/权限。
设置页流程与未知结果处置见[部署手册](../../ops/README.md#首次部署配置)。

已有、残缺、权限异常或符号链接目录均不开放向导，也不覆盖文件。首次初始化最多 300 秒，
只接受无用户对象、无其他客户端的专用空库，使用与普通迁移相同的历史 checksum 兼容层。
失败、中断或超时保留配置/数据库现场；终止信号或无法确认设置终态都不会进入初始化。
不得删除配置强行重跑；核对现场和备份后使用显式维护。普通启动仅校验完整 schema readiness，不隐式迁移。

预检要求三角色同库、同公共文件，Admin/Worker 的 runtime、拍卖策略、control 路径和三组数据密钥一致；
Worker control 必须启用并允许 supervisor UID，监听地址不得冲突。配置格式见[配置手册](../../ops/config/README.md)。
私有运行目录为配置目录同级的 `.<配置目录名>.server`（0700），包含 `control.sock` 和 `owner.lock`。
使用当前服务账号 UID/组，不创建系统账号；不要移动、删除或替换目录绕过锁。
同库只能由同主机、同服务账号管理这一组进程，不能同时运行分体或远端实例。

## 显式远程设置

优先 SSH 隧道；浏览器直接远程设置须显式声明受信 HTTPS 代理：

```bash
./auction-server --config-dir /var/lib/auction/config \
  --allow-remote-setup \
  --setup-origin https://setup.example.com \
  --setup-proxy-ip 127.0.0.1
```

参照[临时 Caddy 站点](../../ops/deploy/caddy/setup.Caddyfile.example)。默认仍 loopback；
非 loopback 的 `--setup-listen` 必须同时启用远程策略，并以防火墙只允许代理访问。
策略检查 TCP peer、精确 Host、`X-Forwarded-Proto: https` 与写请求 Origin；代理须覆盖这些头。
所有 API 逐请求验证部署码且禁止缓存。代理应在本机，远端代理至后端链路另由受信加密隧道保护。
设置后撤掉临时站点，安装正式 Public/Admin 代理；正式 Admin 没有 setup 写接口。

## 生命周期与维护

启动顺序 Worker → Admin → Public，每角色默认等待 readiness 60 秒，
`--startup-timeout-seconds` 范围 1–300。任一退出或启动失败，父进程反序回收其他角色，不无限重启。
SIGINT/SIGTERM 按 Public → Admin → Worker 关闭，总 drain 上限 135 秒；二次信号强制终止。
强杀后逐角色回收最多 5 秒，失败不能报告正常退出。子进程继承部署锁，Linux parent-death signal 防止无主进程。

使用独立[systemd 模板](../../ops/deploy/systemd/auction-server.service.example)：`KillMode=mixed`、
`TimeoutStopSec=155s`，禁止同时启用分体 unit/总控。先以相同账号完成首次设置并正常停止，再启用 unit。
Admin 每次启动撤销旧管理会话，需重新登录，Public 不受影响。

升级先备份并停止整个 supervisor，以同一服务账号执行：

```bash
/opt/auction/bin/auctionctl migrate-managed --config-dir /var/lib/auction/config
```

命令严格预检并取得 supervisor/全部子进程共享锁的独占所有权后才前向迁移；缺少既有锁时拒绝。
它不替代分体 systemd 停机门禁，失败不自动回滚；旧 migrate/deployment 不控制合并 unit。
灾备仍按[独立恢复手册](../../ops/deploy/RECOVERY.md)确认目标与证据。

## 源码 owner 与验证

[src/bootstrap](src/bootstrap) 中，`cli.rs` 拥有参数，`filesystem.rs` 拥有路径/锁，
`configuration.rs` 校验拓扑，`probe.rs` 提供有界 readiness，`supervisor.rs` 管理进程树，`mod.rs` 编排启动。
配置生成、设置服务与空库 SQL 分属 auction-config、auction-setup、auction-storage。

默认仅执行[静态门禁](../../docs/development.md)，首次设置、进程故障和生产切换证据见[验证状态](../../docs/status.md)。
构建通过不证明运行验收完成。
