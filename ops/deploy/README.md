# 部署与发布手册

[文档导航](../../docs/README.md) · [CLI](../../apps/auctionctl/README.md) · [配置](../config/README.md) · [systemd](systemd/README.md) · [恢复](RECOVERY.md) · [v28 接管](LEGACY_V28_ADOPTION.md)

默认按 Public/Admin/Worker 分体部署，可选 Linux/PostgreSQL 单主机 supervisor。
部署前准备真实配置、PostgreSQL 数据库与应用账号；本目录提供审阅模板，不自动安装或覆盖数据库。
以下命令供获准维护窗口执行，阅读或更新文档不授权启动、迁移或行为验证。

## 本页导航

- [可选单文件部署](#可选单文件部署)
- [推荐目录](#推荐目录)
- [发布制品](#发布制品)
- [首次部署配置](#首次部署配置)
- [启动顺序](#启动顺序)
- [数据库恢复与保留](#数据库恢复与保留)
- [反向代理与 systemd](#反向代理与-systemd)

## 可选单文件部署

`just release-check --single-binary` 生成 `target/release-package-single/`，仅含 auction-server、auctionctl
两个可执行文件；server 仍派生三个独立角色进程。以服务账号执行
`auction-server --config-dir /var/lib/auction/config`，仅配置目录明确不存在时开放首次向导。
网页明确确认后才保存、初始化预先准备的专用空库并启动；已有配置普通启动只校验 schema，不隐式迁移。

路径/锁、远程 HTTPS 设置、失败处置和 `migrate-managed` 见[server 手册](../../apps/server/README.md)。
只启用[合并 unit](systemd/auction-server.service.example)，同库不得并行运行分体或远端角色。
合并锁和分体 systemd 停机门禁互不替代；保存响应、静态构建和运行验收须分别判读。

## 推荐目录

分体部署示例：

```text
/opt/auction/bin/                 # Public、Admin、Worker、auctionctl
/etc/auction/common.toml
/etc/auction/public.toml
/etc/auction/admin.toml
/etc/auction/worker.toml
/etc/auction/public.secret.toml
/etc/auction/admin.secret.toml
/etc/auction/worker.secret.toml
/etc/auction/runtime/admin.toml
/var/lib/auction/deployment/       # 总控锁与启动回执
/var/lib/auction/                  # Lite 本地数据库或已配置的私有运行数据
/run/hv-auction/                  # Public/Admin HTTP Unix socket
/run/hv-auction/worker/           # Worker control socket
```

三个 common_file 指向同一只读普通文件；敏感 TOML 仅对应服务账号可读，父目录不得 group/other 可写。
Admin runtime 按 0640 管理。连接串、Cookie、登录 key、数据密钥不得进入 unit、参数、代理配置或日志。

三角色同一数据库目标、可用不同账号。PostgreSQL 使用统一规范主机名或 Unix socket 目录并显式填写 host/port；
预检按 SQLx 解析目标比较，拒绝未知参数和 options 覆盖。Lite 必须绝对本地磁盘路径，拒绝内存库、自定义 VFS 和相对路径。
Admin/Worker 的 runtime 共享、operations 接管及重启范围统一见[配置手册](../config/README.md#runtime-共享与重启)。
恢复后 runtime 指纹与已加载值不同须重新部署重启，不能以旧进程存活证明新配置生效。

## 发布制品

生产使用干净工作树的 `just release-check` 归档，或逐步执行：

```bash
just build-api
just build-release
just verify-artifacts
just package
```

默认分体归档位于 `target/release-package/`，仅包含四程序、配置示例、systemd/Caddy 模板、恢复文档、
retention policy、依赖摘要和 payload 清单。包内 SHA256SUMS 绑定全部批准文件，外层清单绑定 archive/依赖摘要；
不包含数据库、Cookie、.keyenv 或运行数据。敏感模板使用 PostgreSQL 五字段格式，解包后注入真实值。

API-only 固定空 Web 资源集合；内嵌档 Public/Admin 各只含自身 payload，Worker/auctionctl 不含页面。
构建前冻结源码身份，各阶段复查。正式 package 拒绝脏树；`--allow-dirty` 只供受控本地取证，
不能当作上线批准。发布脚本职责见[CI 说明](../../ci/README.md#发布边界)。

[AMD KVM 候选](../build/README.md)是指定 CPUID/ABI 的本地定向构建，不属于通用发行归档；
需分别核对资源身份、目标指令集及运行证据，不能由型号或 cargo check 推导兼容性/性能。

## 首次部署配置

以实际服务账号准备 PostgreSQL 18 空数据库及应用账号，在该账号拥有的受信父目录运行：

```bash
/opt/auction/bin/auction-admin-api setup --output-dir /var/lib/auction/initial-config
```

目标目录必须不存在。也可在无默认配置且未设置 AUCTION_CONFIG_FILE 时直接运行内嵌 PostgreSQL Admin，
生成当前目录的 auction-config/。缺失显式配置、已有残缺目录、符号链接或权限错误不会降级为向导。
分体设置监听 `127.0.0.1:8091/setup`，远程访问用 SSH 隧道；普通 Admin 不注册 setup 写接口。

1. 用终端一次性部署码验证，随后填写数据库五字段、管理员和精确站点 origin；令牌不写浏览器存储。
2. 检查连接与候选设置；检查不写文件，保存仍重新验证。修改参数清除旧检查结果，失效令牌需重新验证并填写密码。
3. 保存公共、角色、敏感与共享 runtime TOML。分体设置保存后退出，不迁移或启动其他角色。
4. 审阅生成文件，安装到正式配置目录；准备运行目录/用户组/unit/代理，再做严格预检和受控迁移启动。

正式 Public/Admin 端口默认 8090/8091，可设 1–65535 且不同，生成 loopback listener；不改变当前 setup 端口或 origin。
同步调整代理/隧道并核对占用和低端口权限，设置检查不会试绑定。
周排期默认关闭，可设 IANA 时区和同一周一至周日周期内的预览/开拍/结束，预览至少提前 120 分钟、开拍早于结束；
不自动跨周，使用所填时区。计划写入 common.toml，runtime 排期接管默认关闭，新设置不改变已有拍卖。

三个设置接口逐请求验证部署码，共享非阻塞操作门禁，每个请求总预算 10 秒，忙碌时不排队连接数据库。
保存超时、连接丢失或终态无法确认时，页面清空令牌/密码并锁定；浏览器未收到成功不代表没有写入。
先停止初始化进程并核对配置目录和数据库现场，不刷新重试、删除或覆盖目录；重新输入部署码不能解除服务端未知状态。

生成器在私有临时目录设定权限，依次同步文件、runtime 子目录和根目录后禁止覆盖地发布，再同步父目录。
发布后结果未知保留现场。合并服务另在明确确认后初始化空库，处理边界见 server 手册。

## 启动顺序

1. 安装三角色/common/敏感/runtime 配置并核对路径和 owner。Worker 必须启用 control，预建运行目录/组，
   allowlist 包含实际 Admin/运维 UID 或 GID。HTTP Unix listener 也须预建目录并配置 socket 组。
2. 审阅三角色 unit 与 auction-stack.service，绑定准确的 AUCTION_CONFIG_FILE；不要分别 enable 三个角色绕过总控。
3. 严格预检并通过总控启动。三角色均 `inactive`/`failed` 且 `MainPID=0` 才可迁移，转换中或停机钩子未完时拒绝。
   迁移后 Worker → Admin → Public 逐个 readiness；全部已运行时 start 只复核顺序/就绪并更新 adopted 回执，部分运行拒绝。
4. 代理只连实际 Public/Admin listener，禁止公开 metrics；Unix upstream 须同步目录权限。

```bash
/opt/auction/bin/auctionctl deployment-check --config-dir /etc/auction --strict-permissions
sudo install -d -m 0700 /var/lib/auction/deployment
sudo /opt/auction/bin/auctionctl deployment start --config-dir /etc/auction
sudo /opt/auction/bin/auctionctl deployment status
sudo /opt/auction/bin/auctionctl deployment restart --config-dir /etc/auction
sudo /opt/auction/bin/auctionctl deployment stop
```

预检不连接数据库。配置目录/common/角色/runtime/敏感文件均禁 symlink；strict-permissions 将权限警告升为失败。
总控状态目录必须预建、规范绝对路径、归其有效用户所有且至多 0700。restart 先预检并反序停机，再迁移和启动；
失败只回滚本次启动的 unit，迁移不自动回退。独立建库/迁移命令见[auctionctl](../../apps/auctionctl/README.md#停机迁移与初始建库)。

每次 Admin 启动永久撤销旧管理会话，需重新登录，Public 不受影响；撤销失败阻断启动。
轮换 actor/secret 前停止所有旧凭据实例并统一新值；恢复旧配置也不会复活已撤销会话。
角色 SIGTERM drain 最多 10 秒，再刷新 exporter 最多 30 秒；分体 unit 预留 45 秒，合并 unit 预算不同。
unit 依赖可能联动其他角色，单角色重启前核对 Requires/After，详见[systemd 手册](systemd/README.md)。

## 数据库恢复与保留

| 任务 | 权威流程 | 边界 |
| --- | --- | --- |
| 当前 schema 备份、离线 verify、空库恢复 | [RECOVERY.md](RECOVERY.md) | 备份身份/哈希、零连接、同名空库、单事务恢复、后验与 receipt |
| 显式破坏性重建 | 同一恢复手册 | 默认只读；独立 execute、目标确认、恢复点和回执，不由启动触发 |
| canonical v28 接管 | [LEGACY_V28_ADOPTION.md](LEGACY_V28_ADOPTION.md) | 专用 manifest、精确格式、bridge 与前向迁移；另导入外部五语言规则 |
| 保留盘点 | 下例与[CLI 说明](../../apps/auctionctl/README.md#保留盘点与恢复预检) | inventory only，不判断删除资格、不归档/删除 |

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl retention-inventory \
    --confirm-database auction_production \
    --output /secure/auction-evidence/retention-inventory.json
```

证据为新绝对路径，父目录预先存在且不可被 group/other 写入。输出绑定[retention policy](retention-policy.v1.json)
SHA-256，含 schema/迁移/关系大小、估算行数、表覆盖及数据族摘要；不锁业务行或修改数据库。
`eligibility_assessed` 和 `eligibility_authorized` 固定 false；保留期限和 mutation 仍需独立设计/授权。
SQLite 的本地文件备份另按 Lite 约束准备，不能套用 PostgreSQL 恢复流程或声称高可用对等。

## 反向代理与 systemd

[Caddy 模板](caddy/Caddyfile)仅压缩 GET/HEAD 的 `/assets/*` 和 `/favicon.ico`，由浏览器 Accept-Encoding
协商 Zstandard/gzip；页面入口、API 和 WebSocket 不在范围。资源所有权、缓存和缺资源响应见
[HTTP 契约](../../docs/contracts/routes.md)。metrics 默认对公网返回 404。

[systemd 模板](systemd/README.md)需替换服务账号/安装路径并核对权限。单角色 unit 不迁移；
分体总控 start/restart 或受停机门禁保护的显式 migrate 才前向迁移。
任何 unit 都不自动备份、恢复、DROP/CREATE 或执行 retention 删除。
