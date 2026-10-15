# 部署与发布手册

[文档导航](../../docs/README.md) · [CLI](../../apps/auctionctl/README.md) · [配置](../config/README.md) · [systemd](systemd/README.md) · [恢复](RECOVERY.md) · [v28 接管](LEGACY_V28_ADOPTION.md)

原生制品仅部署于 Linux x86_64，正式包使用 GNU/glibc ABI；其他 OS/架构及 musl 不在发布承诺内。
默认按 Public/Admin/Worker 分体部署，可选同平台 PostgreSQL 单主机 supervisor。
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

`just release-check --single-binary` 输出 `target/release-package-single/`，可执行文件仅 auction-server、auctionctl；
server 派生三个独立角色进程。服务账号执行 `auction-server --config-dir /var/lib/auction/config`：
仅目录明确不存在时开放向导，网页明确确认后才保存、初始化预备专用空库并启动；普通启动不隐式迁移，只校验 schema。

仅启用[合并 unit](systemd/auction-server.service.example)，同库禁并行分体或远端角色；合并锁与分体停机门禁互不替代。
路径/锁、远程 HTTPS 设置、失败处置及 `migrate-managed` 见[server 手册](../../apps/server/README.md)。
保存响应、静态构建均不代表运行验收。

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
/var/lib/auction/                  # 已配置的私有运行数据
/run/hv-auction/                  # Public/Admin HTTP Unix socket
/run/hv-auction/worker/           # Worker control socket
```

三个 common_file 指向同一只读普通文件；敏感 TOML 仅对应服务账号可读，父目录不得 group/other 可写。
Admin runtime 按 0640 管理。连接串、Cookie、登录 key、数据密钥不得进入 unit、参数、代理配置或日志。

三角色须同一数据库目标，可用不同账号；统一规范主机名或 Unix socket 目录，显式填写 host/port。
目标比较及未知参数/options 拒绝规则见[部署预检](../../apps/auctionctl/README.md#部署预检)。
runtime 共享、operations 接管、恢复指纹变化后的重新部署重启见[配置手册](../config/README.md#runtime-共享与重启)；进程存活不证明配置生效。

## 发布制品

生产使用干净工作树的 `just release-check` 归档，或逐步执行：

```bash
just build-api
just build-release
just verify-artifacts
just package
```

分体归档 `target/release-package/` 仅含四程序、配置示例、systemd/Caddy 模板、恢复文档、retention policy、
依赖摘要和 payload 清单；包内 SHA256SUMS 绑定全部批准文件，外层清单绑定 archive/依赖摘要。
不含数据库、Cookie、.keyenv 或运行数据；解包后向 PostgreSQL 五字段敏感模板注入真实值。

API-only 无 Web 资源；内嵌档 Public/Admin 仅含各自 payload，Worker/auctionctl 无页面。
源码身份须构建前冻结、各阶段复查。正式 package 拒绝脏树；`--allow-dirty` 仅供受控本地取证，不是上线批准。
完整发布门禁见[CI 说明](../../ci/README.md#发布边界)。

后端沿用[根 release profile](../../Cargo.toml)：opt-level=3/Fat LTO/单 CGU、panic 展开；通用包禁用构建机 `native`。
WASM 以体积为先；LTO 增加构建时间和内存，不证明性能收益。
[AMD KVM 定向候选](../build/README.md)不进入通用归档，须核对资源身份、CPUID/ABI 和运行证据；型号或 cargo check 不证明兼容性/性能。

## 首次部署配置

以实际服务账号准备 PostgreSQL 18 空数据库及应用账号，在该账号拥有的受信父目录运行：

```bash
/opt/auction/bin/auction-admin-api setup --output-dir /var/lib/auction/initial-config
```

目标目录必须不存在。无默认配置且未设置 AUCTION_CONFIG_FILE 时，直接运行内嵌 PostgreSQL Admin 可生成当前目录的 auction-config/；
显式配置缺失、残缺目录、符号链接或权限错误均不降级为向导。设置监听 `127.0.0.1:8091/setup`，远程用 SSH 隧道；正式 Admin 无 setup 写接口。

1. 验证终端一次性部署码，填写数据库五字段、管理员和精确站点 origin；令牌不存浏览器。
2. 检查连接/候选（不写文件），保存时重新验证；改参数即清除旧检查，令牌失效须重新验证并填写密码。
3. 保存公共、角色、敏感及共享 runtime TOML 后退出，不迁移或启动其他角色。
4. 审阅并安装配置，按[启动顺序](#启动顺序)准备目录/用户组/unit/代理、严格预检及受控迁移启动。

正式 Public/Admin 端口默认 8090/8091，可设 1–65535 且不同，生成 loopback listener；不改变当前 setup 端口或 origin。
同步调整代理/隧道并核对占用和低端口权限，设置检查不会试绑定。
周排期默认关闭，可设 IANA 时区和同一周一至周日周期内的预览/开拍/结束，预览至少提前 120 分钟、开拍早于结束；
不自动跨周，使用所填时区。计划写入 common.toml，runtime 排期接管默认关闭，新设置不改变已有拍卖。

三个设置接口逐请求验证部署码，共享非阻塞门禁；每请求总预算 10 秒，忙碌时不排队连接数据库。
保存超时、连接丢失或终态不明时，页面清空令牌/密码并锁定；未收到成功不代表未写入。
须停止初始化进程、核对配置目录及数据库；禁止刷新重试、删除或覆盖目录，重输部署码不能解除服务端未知状态。

生成器在私有临时目录设权限，依次同步文件、runtime 子目录、根目录，禁止覆盖地发布后同步父目录；
发布后结果未知须保留现场，详见[生成器边界](../../crates/auction-config/README.md#解析与装配入口)。
合并服务的确认与空库初始化见[server 手册](../../apps/server/README.md#首次启动)。

## 启动顺序

1. 安装三角色/common/敏感/runtime 配置，核对路径与 owner；启用 Worker control，预建目录/组，allowlist 纳入实际 Admin/运维 UID 或 GID。HTTP Unix listener 同样预建目录/socket 组。
2. 审阅三角色 unit 与 auction-stack.service，准确绑定 AUCTION_CONFIG_FILE；只 enable 总控。
3. 严格预检后由总控迁移、启动：三角色须 `inactive`/`failed` 且 `MainPID=0`，转换中或停机钩子未完即拒绝；随后 Worker → Admin → Public 逐个 readiness。全部已运行仅复核顺序/就绪并更新 adopted 回执，部分运行拒绝。
4. 代理仅连实际 Public/Admin listener，禁止公开 metrics；Unix upstream 同步目录权限。

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

Admin 每次启动永久撤销旧管理会话，失败即阻断启动；须重新登录，Public 不受影响。
轮换 actor/secret 前停掉所有旧凭据实例并统一新值；恢复旧配置不复活会话。
SIGTERM：drain 最多 10 秒、exporter 最多 30 秒，分体 unit 45 秒；合并及外层预算见[关闭预算](systemd/README.md#关闭预算)。
单角色重启可能联动依赖角色，操作前核对 Requires/After。

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

## 反向代理与 systemd

[Caddy 模板](caddy/Caddyfile)仅压缩 GET/HEAD 的 `/assets/*` 和 `/favicon.ico`，由浏览器 Accept-Encoding
协商 Zstandard/gzip；页面入口、API 和 WebSocket 不在范围。资源所有权、缓存和缺资源响应见
[HTTP 契约](../../docs/contracts/routes.md)。metrics 默认对公网返回 404。

[systemd 模板](systemd/README.md)安装前须替换账号/路径并核对权限；迁移仅由总控 start/restart 或受停机门禁保护的显式 migrate 执行，见[独立迁移门禁](systemd/README.md#独立迁移门禁)。任何 unit 均不自动备份、恢复、DROP/CREATE 或执行 retention 删除。
