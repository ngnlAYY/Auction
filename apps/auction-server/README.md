# auction-server

[文档导航](docs/README.md) · [架构](docs/design/architecture.md) · [部署手册](ops/README.md)

Linux x86_64/PostgreSQL 单主机的可选单文件服务：supervisor 用同一可执行文件派生 Worker、Admin、Public，
复用各 app 的库装配，保留独立进程、路由和权限边界。`auctionctl` 仍独立，默认分体发布不变。
装配入口在编译期拒绝其他 OS/架构；浏览器 WASM 保留，默认发布 ABI 见[开发指南](docs/guide/development.md#工具与依赖来源)。

## 构建与文件

```bash
just build-single
just release-check --single-binary
```

`build-single` 可在开发工作树构建并核验资源边界，正式 `release-check` 要求干净、已提交源码，均不运行测试。
二进制与归档位于 `target/dist/artifacts/single/bin/`、`target/dist/package-single/`，可执行文件白名单仅
`auction-server`、`auctionctl`；无参数 release-check 仍生成四程序分体包。`postgres` 启用完整后端，
`web-embed` 包含两个独立 Web payload，`cache-redis` 可选。

## 首次启动

先准备 PostgreSQL 18 专用空库和应用账号，以最终服务账号在其拥有、不可被其他账号写入的父目录执行
`./auction-server --config-dir /var/lib/auction/config`。`--config-dir` 默认 `auction-config`，
相对当前目录解析；设置了 `AUCTION_CONFIG_FILE`（包括空值）会被拒绝。只有目标目录明确不存在时才开放
`http://127.0.0.1:8091/setup` 并打印一次性部署码，网页确认专用空库初始化后先原子发布配置、再初始化数据库、
最后顺序启动角色；保存响应只证明落盘，启动结果看各角色 readiness。
已有、残缺、权限异常或符号链接目录都不开放向导、也不覆盖文件。第一次初始化最多 300 秒，只接受无用户对象、
无其他客户端的专用空库；失败或无法确认终态时保留现场、须人工核对，不得删除配置重跑。
普通启动仅校验完整 schema readiness，不隐式迁移，向导流程见[首次部署配置](ops/README.md#首次部署配置)。

预检要求三角色同库、同公共文件，runtime 同源且装备规则与 `bot_players` 名单快照一致（或全部省略）；装备数据密钥的标识与材料三角色一致，
用户登录/外部凭据密钥、拍卖策略及 control 路径只要求 Admin/Worker 一致。Admin 已配置的 `worker_config_file` 须绑定实际 Worker 角色文件。
Worker control 须启用并允许 supervisor UID，监听地址不得冲突；Public/Admin 端口默认 8090/8091，
可选 1–65535 且不同、固定 loopback，须自行同步代理并核对端口/权限。字段见[配置手册](ops/config/README.md)。
Unix 入口预检只接受不存在的路径，或由当前服务账号拥有且连接明确拒绝的旧 socket；探测期间路径消失也可继续。
活动连接、非 socket、属主不符、其他连接错误、100 毫秒探测超时或旧 socket 被替换均阻止启动和首次空库初始化。
预检不删除路径，绑定器仍重新核验并处理旧 socket；预检完成后仍须保持目录稳定。
私有运行目录为配置目录同级的 `.<配置目录名>.server`（0700），含 `control.sock` 和 `owner.lock`；
使用当前服务账号 UID/组，不创建系统账号，也不要移动、删除或替换目录绕过锁——同库只能由同主机、
同服务账号管理这一组进程，不能同时运行分体或远端实例。

## 显式远程设置

默认仍 loopback，优先 SSH 隧道；浏览器直接远程设置须显式声明精确 HTTPS origin 与受信代理 IP，
即 `--allow-remote-setup --setup-origin https://setup.example.com --setup-proxy-ip 127.0.0.1`。
非 loopback 的 `--setup-listen` 必须同时启用远程策略，并以防火墙只允许代理访问。策略检查 TCP peer、精确 Host、
`X-Forwarded-Proto: https` 与写请求 Origin，代理须覆盖这些头；所有 API 逐请求验证部署码且禁止缓存。
参照[临时 Caddy 站点](ops/deploy/caddy/setup.Caddyfile.example)，设置后撤掉临时站点并安装正式 Public/Admin 代理；
正式 Admin 没有 setup 写接口。

## 生命周期与维护

启动顺序 Worker → Admin → Public，每角色默认等待 readiness 60 秒，`--startup-timeout-seconds` 范围 1–300；
任一退出或启动失败，父进程反序回收其他角色，不无限重启。SIGINT/SIGTERM 按 Public → Admin → Worker 关闭，
总 drain 上限 135 秒，二次信号强制终止；强杀后逐角色回收最多 5 秒。任一角色在关闭时非零退出也会使
supervisor 失败退出，但仍先回收其余角色，不把 drain 失败报告为正常停止；强制回收中单个 wait 失败
或超时也会继续等待其他角色，最终传播失败。
readiness 仅接受 512 字节内由 CRLF 完整终止的 HTTP/1.0 或 HTTP/1.1 200 状态行；
分片到达的 CRLF 可正常识别，EOF、仅 LF、未终止满缓冲区及非 200 响应均不算就绪。
子进程继承部署锁，Linux parent-death signal 防止无主进程。使用独立
[systemd 模板](ops/deploy/systemd/auction-server.service.example)，禁止同时启用分体 unit/总控，安装与探测门禁见
[systemd 手册](ops/README.md#systemd-服务与顺序控制)；Admin 每次启动撤销旧管理会话、需重新登录，Public 不受影响。

升级须先备份并停止整个 supervisor，再以同一服务账号执行
`auctionctl migrate-managed --config-dir /var/lib/auction/config`：它严格预检并取得 supervisor/全部子进程
共享锁的独占所有权后才前向迁移，缺少既有锁时拒绝，失败不自动回滚；旧 migrate/deployment 不控制合并 unit。
参数与分体停机门禁见 [auctionctl](apps/auctionctl/README.md#停机迁移与初始建库)，灾备见[恢复手册](ops/deploy/RECOVERY.md)。

## 源码 owner 与验证

[src/bootstrap](apps/auction-server/src/bootstrap) 中，`cli.rs` 拥有参数，`filesystem.rs` 拥有路径/锁，`configuration.rs` 校验拓扑，
`probe.rs` 提供有界 readiness，`supervisor.rs` 管理进程树，`mod.rs` 编排启动；配置生成、设置服务与空库 SQL
分属 auction-config、auction-setup、auction-storage。默认仅执行[静态门禁](docs/guide/development.md)，
首次设置、进程故障和生产切换证据见[验证状态](docs/status.md)。
