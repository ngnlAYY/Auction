# auction-server

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [部署手册](../../ops/deploy/README.md)

Linux/PostgreSQL 单主机的可选单文件服务。默认分体部署不变；`auctionctl` 仍是独立运维程序。
单文件不等于单进程：一个 supervisor 用同一个可执行文件派生 Worker、Admin、Public 三个子进程。
三个角色复用各 app 的库入口，HTTP/业务/存储边界与日志、信号处理不合并；不依赖 `auctionctl`。

## 构建与文件

```bash
# 编译并核验两种资源边界，不要求工作树干净。
just build-single
# 正式归档必须使用干净、已提交的源码；不运行测试。
just release-check --single-binary
```

二进制在 `target/release-artifacts/single/bin/`；归档在 `target/release-package-single/`，可执行文件
白名单只有 `auction-server` 和 `auctionctl`。默认无参数 `just release-check` 仍生成原四二进制包。
`postgres`、`web-embed` 分别启用完整后端与两个独立 Web payload；`cache-redis` 可选，默认缓存禁用。

## 首次启动

先创建 PostgreSQL 18 专用空数据库和应用账号。用最终运行服务的同一个账号，在它拥有且不可被
其他账号写入的父目录执行：

```bash
./auction-server --config-dir /var/lib/auction/config
```

`--config-dir` 默认 `auction-config`，相对当前目录解析。顶层 `auction-server` 检测到
`AUCTION_CONFIG_FILE` 已设置（包括空值）会拒绝启动，不是静默忽略；启动前须移除该环境变量。
仅配置目录明确不存在时开放 `http://127.0.0.1:8091/setup`，终端打印一次性部署码。
验证部署码后填写数据库、管理员、两个精确站点 origin 和其余设置。正式 Public/Admin 监听端口
可分别设置为 1–65535，默认 8090/8091 且必须不同；始终绑定 `127.0.0.1`，不改变当前设置入口、
站点 origin 或代理配置。需自行调整反向代理/SSH 隧道并核对端口占用及权限，表单检查不试绑定端口。
勾选专用空库初始化确认，
点击“确认、保存并启动”：先原子生成配置，再由 supervisor 初始化空库并顺序启动角色。
保存响应仅证明配置落盘，不代表迁移或服务已经就绪；以终端各角色 `ready` 为准，页面提供管理入口链接。
首次初始化与普通迁移共用保留历史 checksum 的别名兼容层；设置的成功、拒绝和超时应用响应
统一禁止缓存与嵌入。请求头预算 5 秒，正文/异步处理预算 10 秒，停止后排空最多 10 秒；
终止信号或无法确认终态都不会自动进入初始化。修复边界见[状态说明](../../docs/status.md#源码核对发现的实现缺口)，
仍不表示该链路已运行验收。正式 Admin 每次启动还会永久撤销旧管理会话，需重新登录，Public 不受影响。

配置目录已有、残缺、权限错误或是符号链接时不开放向导，也不覆盖任何文件。
迁移失败、中断、超时均保留配置与数据库现场，下一次普通启动不会隐式重试迁移。不得删除配置来
强行重跑初始化；核对现场和备份后按下文执行显式维护。首次初始化总预算为 300 秒，只接受无用户对象、无其他客户端的
专用空库，不能接管旧库。普通启动先检查完整 schema readiness，未通过时不启动任何角色。

公共、三角色、敏感和 runtime TOML 格式不变。启动预检要求三角色同库、同一公共配置，
Admin/Worker 的 runtime、拍卖策略、control 路径和装备/用户登录/外部凭据数据密钥匹配；
Worker control 必须启用并允许 supervisor 的 UID，三角色监听地址不能重叠。
合并向导把 Worker control 放在配置目录同级的
`.config.server/control.sock`，该私有运行目录权限为 `0700`；实际名称为 `.<配置目录名>.server`。
默认使用当前服务账号的 UID 和组，不创建系统账号/用户组。此目录持有 `owner.lock`，不要删除、
替换或移动配置/运行目录以绕过实例锁。只支持同一主机、同一服务账号管理这一组进程，不支持与
分体部署或远端实例同时访问同一数据库。

## 显式远程设置

优先使用 SSH 隧道。需要浏览器直接远程访问时，显式声明受信 HTTPS 代理：

```bash
./auction-server --config-dir /var/lib/auction/config \
  --allow-remote-setup \
  --setup-origin https://setup.example.com \
  --setup-proxy-ip 127.0.0.1
```

模板见 [临时 Caddy 站点](../../ops/deploy/caddy/setup.Caddyfile.example)。仍默认 loopback；
`--setup-listen` 可改变监听地址，但非 loopback 必须同时开启远程策略，并由防火墙限制到代理。
远程策略校验 TCP peer、精确 Host、代理提供的 `X-Forwarded-Proto: https` 和写请求 Origin；
代理必须覆盖这些转发头，不能直接转发客户端伪造值。任何 API 仍逐请求验证部署码，响应禁止缓存。
不要把凭据发往明文网络：代理应在本机，远端代理的后端链路须另由受信加密隧道保护。
设置完成后撤掉临时代理站点，安装正式 Public/Admin 反向代理；正式 Admin 不注册 setup 写接口。

## 生命周期与维护

启动顺序为 Worker → Admin → Public；每个角色等待 readiness 默认 60 秒，
`--startup-timeout-seconds` 可选范围 1–300。任一角色退出或启动失败，父进程反序停止并回收其余角色，
不无限重启。SIGINT/SIGTERM 触发 Public → Admin → Worker 顺序关闭，总 drain 上限 135 秒；
第二次信号强制终止。强杀后逐角色回收最多 5 秒，未能回收则失败，绝不报告正常退出。
子进程继承同一部署锁，Linux parent-death signal 防止父进程意外死亡后留下无主业务进程。

systemd 使用独立的 [auction-server.service 模板](../../ops/deploy/systemd/auction-server.service.example)，
`KillMode=mixed`、`TimeoutStopSec=155s`。不要同时启用 `auction-stack.service` 或三个分体 unit。
先手动完成首次配置并确认正常退出后再启用 unit；模板不自动重启失败的首次迁移。

升级前备份并停止整个 supervisor，用相同服务账号执行：

```bash
/opt/auction/bin/auctionctl migrate-managed --config-dir /var/lib/auction/config
```

该命令做严格配置预检，并取得 supervisor 及全部子进程共享的独占锁才运行 forward-only 迁移；
缺少既有合并部署锁时拒绝，不能用它替代分体 systemd 停机门禁。升级迁移不自动回滚。
旧 `migrate`/`deployment` 仍属于分体部署，不控制合并 unit；危险灾备操作仍须按独立手册逐项确认。

## 源码 owner 与验证

`src/bootstrap/cli.rs` 拥有参数，`filesystem.rs` 拥有路径和进程锁，`configuration.rs` 负责角色拓扑预检，
`probe.rs` 负责有界 readiness，`supervisor.rs` 负责子进程树，`mod.rs` 只编排首次设置和启动。
配置生成由 `auction-config` 拥有，首次设置由 `auction-setup` 拥有，空库 SQL/迁移由 `auction-storage` 拥有。
保护不覆盖配置、未知结果不重试、迁移前无业务进程、三角色同库、业务写入沿用原事务/审计/outbox 等不变量。

本次默认只进行格式、编译、Clippy、文档/策略/i18n 与制品静态核验；未进行浏览器、真实数据库、
进程故障注入或生产切换验收，不因构建通过宣称运行验证完成。
