# 部署与发布手册

[文档导航](../../docs/README.md) · [CLI 参数](../../apps/auctionctl/README.md) · [发布装配](../../ci/release.sh) · [systemd 模板](systemd/README.md) · [备份与恢复](RECOVERY.md) · [v28 接管](LEGACY_V28_ADOPTION.md)

按当前三进程边界说明安装、预检、顺序、制品和运维入口；常规部署前必须准备真实配置与维护环境，
PostgreSQL 单机首次部署可通过独立设置页生成配置，但仍需先准备 PostgreSQL 数据库和应用账号。

本目录只描述当前 Rust 三进程边界的部署契约，不提供安装脚本，也不会在部署时改写配置
字段或自动覆盖数据库。外部脚本或未登记的单体启动流程不能直接套用。

## 本页导航

- [推荐目录](#推荐目录)
- [发布制品](#发布制品)
- [首次部署配置](#首次部署配置)
- [启动顺序](#启动顺序)
- [数据库恢复与保留](#数据库恢复与保留)
- [反向代理与 systemd](#反向代理与-systemd)

## 推荐目录

生产环境可以采用以下目录布局；实际安装路径可以改变，但同一部署中的配置引用、权限和
反向代理上游必须保持一致：

```text
/opt/auction/bin/auction-public-api
/opt/auction/bin/auction-admin-api
/opt/auction/bin/auction-worker
/opt/auction/bin/auctionctl
/etc/auction/common.toml
/etc/auction/public.toml
/etc/auction/admin.toml
/etc/auction/worker.toml
/etc/auction/public.secret.toml
/etc/auction/admin.secret.toml
/etc/auction/worker.secret.toml
/etc/auction/runtime/admin.toml
/var/lib/auction/deployment/       # systemd 总控锁与无敏感信息的启动回执
/var/lib/auction/                  # SQLite Lite 时还保存本地数据库文件
/run/hv-auction/                  # Public/Admin HTTP Unix socket 与运行时目录
/run/hv-auction/worker/           # Worker control socket 等运行时文件
```

三个角色文件必须引用同一份只读 `common.toml`；`*.secret.toml` 只能由对应运行账号读取，父目录
不得允许 group/world 写入；Admin runtime 文件只保存非敏感设置，权限按 `0640` 管理。不要把连接串、Cookie、登录 key 或数据密钥
写入 unit 文件、命令行参数、Caddyfile 或日志。

管理页接管 `operations` 任一组前，先安装支持该文档的 Admin/Worker，并让两者引用同一份
runtime 文件；部署预检会拒绝路径不同或有效拍卖模板/排期不一致的组合。六组 `managed` 默认
关闭，保留已有部署参数。保存经济模板或排期后需重启 Admin 与 Worker；论坛、告警和 Worker
吞吐组及邮件调度/处理组只需重启 Worker。保存不会热改当前进程，也不会回写既有拍卖快照或持久任务。
邮件调度由 `operations.mail` 接管时仍保留部署侧的远程地址、凭据、邮箱身份和全局启用开关。
若 Admin 启动时报告 runtime 文件与已加载配置不一致，说明恢复或并发编辑已改变文件；先核对
恢复后的候选，再重新执行部署重启，不能把旧进程的存活视为新配置已经生效。

三个角色还必须指向同一数据库目标，允许使用不同数据库账号。PostgreSQL 在各敏感 TOML 中显式
填写 `[database]` 的 host、port、username、password、name（或使用兼容 URL），使用同一规范主机名或同一实际 Unix socket 目录；部署预检按 SQLx
解析后的目标比较，拒绝未知参数和 `options` 会话覆盖。SQLite 部署使用绝对磁盘路径；相对路径
会因 CLI/systemd 的工作目录可能不同而被拒绝，内存库与自定义 VFS 也不能用于三进程部署。

## 发布制品

生产安装应使用 `just release-check` 在干净工作树生成的 PostgreSQL 完整档 archive，而不是直接复制
某次 `cargo build` 的 `target/release`。发布包只包含四个内嵌发布二进制、
配置/敏感配置示例、systemd/Caddy 模板、恢复文档、retention policy、依赖摘要和 Web payload 清单；
不会包含数据库、Cookie、`.keyenv`、日志或运行时目录。archive 内 `SHA256SUMS` 绑定全部批准文件，外层
`target/release-package/SHA256SUMS` 再绑定 archive 与 `DEPENDENCIES.md`。
完整档归档中的 `config/examples/secrets.toml.example` 使用 PostgreSQL `[database]` 五字段模板；
填写主机、端口、账号、密码和库名即可，由配置层生成内部连接串。

```bash
just build-api
just build-release
just verify-artifacts
just package
```

`build-api` 明确关闭 `web-embed`，即使本机已有前端文件也只生成静态路由 503 的 API-only 二进制；
`build-release` 只让 Public 嵌入 Public payload、Admin 嵌入 Admin payload，Worker/`auctionctl` 不含
两端资源。正式 `package` 拒绝脏工作树；`--allow-dirty` 只供迁移期间受控取证，输出名称、依赖摘要和
源码指纹都会保留 dirty 事实，不得作为正式上线批准。解包后仍需按下文把示例复制到 `/etc/auction`、
注入真实敏感文件并完成目标主机权限/readiness 演练。

## 首次部署配置

内嵌 PostgreSQL 制品支持 `auction-admin-api setup --output-dir <绝对路径>`；也可在没有默认配置
且未设置 `AUCTION_CONFIG_FILE` 时直接运行 `auction-admin-api`，输出目录为当前目录下的
`auction-config/`。目标目录必须不存在，父目录须由运行账号拥有；先准备 PostgreSQL 18 数据库
和应用账号，再以实际服务账号运行初始化进程，确保配置 owner 和 Worker control UID 与部署一致。
设置页只监听 `127.0.0.1:8091`，远程访问使用 SSH 隧道。第一步填写终端输出的一次性部署码
（设置令牌），点击“验证并进入下一步”；只有服务端验证通过后才显示数据库、管理员和基础/进阶
配置。部署码验证本身不连接数据库或写入文件；错误时可核对当前进程的终端输出后重试。
页面刷新后需重新验证，部署码不会写入浏览器存储；后续配置校验和保存仍逐次验证部署码。

第二步按数据库、管理员和基础参数填写；管理员密码需确认两次，确认值只留在浏览器。每项说明
列出格式、数值范围或 UTF-8 字节长度，进阶参数默认折叠且预填默认值。检查和保存都会先检查输入，
错误摘要可跳到对应字段，进阶字段有错时自动展开。修改参数会清除上次检查结果；“检查连接和设置”
不写入文件，保存仍会重新检查。令牌失效后返回第一步，并清空所有密码，重新验证后需要再次填写。
三个接口的每次请求各有 10 秒总预算，并非三个步骤合计 10 秒；保存超时、连接丢失或返回内容
无法确认时，页面会清空部署码和所有密码，锁定验证、检查、保存及输入控件，并保留结果不确定
提示。先停止初始化进程、核对目标配置目录和进程状态，不要刷新后直接重试、删除或覆盖目录；
当前页面不提供解除锁定按钮。浏览器未收到成功响应不代表服务端未写入。

初始化进程内三个接口共用非阻塞操作门禁；其他页面正在检查或保存时返回忙碌提示，不排队连接
数据库。服务端若在目录发布后无法确认持久化结果，会保持结果不确定状态，禁止后续校验或保存；
重新输入部署码不会解除该状态，应停止初始化进程后核对目标目录，不能把它当成普通连接失败重试。
默认配置路径若已是目录、悬空符号链接，或因权限等原因无法探测，也不会自动开放设置页。

配置生成复用通用模板，写出公共、三角色、敏感和共享 runtime TOML。临时目录在创建时即限制
访问权限，发布前依次同步所有文件、`runtime/` 子目录及临时根目录；发布后同步目标父目录，
目标名称始终禁止覆盖，发布后结果不确定时保留目标而不回滚。保存成功后初始化进程退出；
随后按下文准备运行目录、Worker control 组和 systemd unit，审阅 Caddy 配置，并通过
`auctionctl deployment-check --strict-permissions` 与总控完成迁移和启动。

迁移前只有 systemd `inactive`/`failed` 且 `MainPID=0` 才表示停止完成；`deactivating` 和停止钩子
仍在执行时不能开始迁移。`restart` 会等待 systemd stop 完成并复核严格停止状态。

## 启动顺序

1. 将公共配置、三个角色配置和独立敏感 TOML 放到受信目录，并确保三个 `common_file` 都解析到
   同一份普通非符号链接文件；生产 Worker 必须启用
   `[worker.control]`，预建配置中的 runtime 目录与 Unix group，并把实际 Admin/运维 UID 或 GID
   写入 allowlist，以提供经过内核身份认证的 Unix socket readiness。若 Public/Admin 选择 Unix
   listener，也必须预建对应 runtime 目录，并让 unit 运行账号属于配置的 socket group。
2. 安装并审阅三个角色 unit 与 `auction-stack.service`，确保 unit 中的
   `AUCTION_CONFIG_FILE` 分别精确指向 `/etc/auction/public.toml`、`admin.toml` 和
   `worker.toml`。
3. 通过 `auctionctl deployment start` 或启用 `auction-stack.service`。总控会执行严格配置
   预检、TCP/Unix listener 冲突检查、Admin/Worker control socket 与数据密钥一致性检查和
   forward-only migration，再按 Worker → Admin → Public 顺序逐个等待实际 listener 的
   readiness；失败只回滚本次启动的 unit，migration 不自动回退。
4. 反向代理只连接 Public/Admin 的 TCP 地址或 Unix socket；不要把 `/metrics` 暴露给公网，
   Caddy 模板已默认返回 `404`。切换到 Unix upstream 时要同步修改 Caddy 上游值和 unit 的
   runtime 目录权限。

只读配置预检可单独运行 `auctionctl deployment-check --config-dir /etc/auction`；默认会报告
目录、公共配置、角色配置、runtime 和敏感文件的权限警告，`--strict-permissions` 会把这些权限边界作为失败
条件。无论是否严格，配置目录、公共/角色配置、runtime 和敏感 TOML 都不能使用符号链接。

示例命令（实际部署应使用已安装的二进制，而不是 `cargo run`）：

```bash
sudo install -d -m 0700 /var/lib/auction/deployment
sudo /opt/auction/bin/auctionctl deployment start --config-dir /etc/auction
sudo /opt/auction/bin/auctionctl deployment status
sudo /opt/auction/bin/auctionctl deployment restart --config-dir /etc/auction
sudo /opt/auction/bin/auctionctl deployment stop
```

`AUCTION_CONFIG_FILE` 只用于 unit 选择主配置文件，不能覆盖单个配置项。systemd 拥有单角色的
PID、信号、`TimeoutStopSec`、退出码、自动重启和 journald；Rust 总控只拥有跨角色顺序、严格预检、
应用 readiness、失败回滚、互斥锁和私有启动回执。默认状态目录是
`/var/lib/auction/deployment`，必须是已存在、规范化、归总控当前有效用户所有且权限至多 `0700`
的绝对路径；配置目录和三个主 TOML 不得是符号链接或允许 group/other 写入。完整 unit
安装方式见 [`systemd/README.md`](systemd/README.md)。

单角色收到 systemd 的 SIGTERM 后先执行最多 10 秒的 HTTP/Worker drain，再按
`tracing.shutdown_timeout_seconds` 刷新 exporter；该字段允许的最大值为 30 秒。因此示例 unit
使用 `TimeoutStopSec=45s`，为最长关闭路径保留 5 秒进程回收余量。不要把该值缩短到两段上限之
和以下；排空期间再次发送 SIGINT/SIGTERM 会立即强制收口，但 exporter 刷新仍由自身超时保护。

## 数据库恢复与保留

生产 PostgreSQL 的备份、恢复、保留期限和 destructive rebuild 必须由部署方先确定权限、
目标数据库和维护窗口。当前 Rust 已提供迁移目标保护、应用 readiness、
`auctionctl retention-inventory` 只读盘点，以及不修改源库的 `postgres-backup`/
`postgres-backup-verify` 和只向空库写入的 `postgres-restore`；盘点加载版本化的
[`retention-policy.v1.json`](retention-policy.v1.json)，输出政策 SHA-256、schema/迁移/关系大小、
行数估计、政策表覆盖和数据族汇总，但不判断保留资格。`postgres-backup` 依赖部署环境提供
`pg_dump`/`pg_restore`，输出 custom-format archive 和带数据库身份摘要、schema 版本及 SHA-256
的不可覆盖 manifest；`postgres-backup-verify` 可离线重新校验配对文件。恢复命令使用单事务、
空库/连接检查、artifact 二次校验、恢复后 schema 证明和不可覆盖 receipt；详细流程见
[`RECOVERY.md`](RECOVERY.md)。宿主机 start/stop/restart/status 总控已经迁移；数据库
destructive rebuild 也已由独立的 `auctionctl postgres-rebuild` 契约实现，默认只读，并要求
verified backup 或一次性环境豁免、双重目标确认、零连接、advisory lock 和私有回执。它不会由
启动、重启、迁移或任何 unit 隐式执行。
canonical PostgreSQL v28 历史格式使用独立的备份格式、严格表/投影预检和显式接管命令，不能交给
普通 `migrate` 或当前 backup/restore 猜测处理；完整停机、备份、确认、接管与失败恢复流程见
[`LEGACY_V28_ADOPTION.md`](LEGACY_V28_ADOPTION.md)。来源配置目录中的五语言规则不一定存在于 v28
数据库，必须按同一手册在启动前保存并经 `import-rules-directory` 显式导入。
当前 retention 契约只允许 inventory 并明确禁止 mutation；未来完整 retention policy 和 mutation
需要独立设计与授权。

只读盘点示例：

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl \
    retention-inventory \
    --confirm-database auction_production \
    --output /secure/auction-evidence/retention-2026-08-23.json
```

证据文件必须是新的绝对路径，父目录必须已存在且不得允许 group/other 写入；命令不会覆盖已有
文件、执行迁移、锁业务行或修改数据库。

恢复前只读预检示例：

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl \
    recovery-preflight \
    --confirm-database auction_restore \
    --output /secure/auction-evidence/recovery-preflight-2026-08-23.json
```

预检只确认目标连接、主节点可写状态、其他连接、用户对象、schema identity 和迁移历史；
`restore_preflight_passed=true` 不代表 archive 已验证或可以直接恢复。它不会停止进程、执行
`pg_restore`、创建/删除数据库或覆盖已有对象。

备份示例：

```bash
AUCTION_CONFIG_FILE=/etc/auction/worker.toml \
  /opt/auction/bin/auctionctl \
    postgres-backup \
    --confirm-database auction_production \
    --output /secure/auction-backups/auction-2026-08-23.dump

/opt/auction/bin/auctionctl \
  postgres-backup-verify \
  --confirm-database auction_production \
  --manifest /secure/auction-backups/auction-2026-08-23.dump.manifest
```

备份命令只允许新的绝对路径，拒绝覆盖已有 archive/manifest；源库必须是当前 Auction schema，
并在 `pg_dump` 前后保持同一迁移版本。恢复必须由操作员先停止外部进程并准备同名空库。
备份/恢复消费配置层生成的数据库连接；`[database]` 五项或兼容 `database_url` 必须显式提供
host 与 port（默认端口也写 `5432`），
外部工具不会继承 `PGHOST`、`PGHOSTADDR` 或 `PGPORT` 来选择目标。
`postgres-restore` 不会终止连接、清理现有 schema 或破坏性重建数据库。需要由仓库工具准备空库时，
必须单独显式运行 `postgres-rebuild --mode empty-for-restore`，完整确认和回执流程见
[`RECOVERY.md`](RECOVERY.md)。

v28 历史格式的 `postgres-legacy-v28-backup`、离线 verify 和 adoption 使用另一种 manifest。该格式
故意不被 `postgres-backup-verify`/`postgres-restore` 接受；接管恢复点不能误当成已经带 SQLx
历史的当前 Rust 备份。

SQLite 只用于本地或单主机 Lite 部署：数据库文件必须位于本地文件系统，每个角色至多一
个实例，Worker 并发为一。不能用本目录的 PostgreSQL 备份流程把 SQLite 当作生产高可用
替代品。


## 反向代理与 systemd

反向代理示例见 [`caddy/Caddyfile`](caddy/Caddyfile)。
Public/Admin 模板只对 `GET`/`HEAD` 请求中的 `/assets/*` 和 `/favicon.ico` 启用 Zstandard/gzip 响应压缩，
由 Caddy 按浏览器的 `Accept-Encoding` 与默认内容类型、大小条件协商。压缩覆盖当前内嵌发布的
指纹 JS、WASM 和 CSS；浏览器页面入口、API 与 WebSocket 不在匹配范围内。静态资源的
内嵌所有权、API-only 制品的缺失资源响应以及 HTTP 缓存策略仍由 Rust 服务按
[`routes.md`](../../docs/contracts/routes.md) 契约提供。

systemd 服务模板见 [`systemd/README.md`](systemd/README.md)；模板必须先替换运行账号和安装路径。
三个角色 unit 不自动迁移；只有显式启动总控，或在确认三个角色均已停止后显式调用受保护的
`auctionctl migrate`，才会执行 forward-only migration。任何 unit 都不会自动备份、恢复、
DROP/CREATE 数据库或执行 retention 删除。
