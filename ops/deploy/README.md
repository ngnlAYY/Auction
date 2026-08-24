# Auction Rust 部署资料

本目录只描述当前 Rust 三进程边界的部署契约，不是通用安装器，也不会在部署时改写配置
字段或自动覆盖数据库。旧版 `Auction-BF/ops/deploy` 依赖 JSON、环境变量和旧的单体启动
流程，不能直接复制到本项目。

## 推荐目录

生产环境可以采用以下目录布局；实际安装路径可以改变，但同一部署中的配置引用、权限和
反向代理上游必须保持一致：

```text
/opt/auction/bin/auction-public-api
/opt/auction/bin/auction-admin-api
/opt/auction/bin/auction-worker
/opt/auction/bin/auctionctl
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

`*.secret.toml` 只能由对应运行账号读取，父目录不得允许 group/world 写入；Admin runtime
文件只保存非敏感设置，权限按 `0640` 管理。不要把连接串、Cookie、登录 key 或数据密钥
写入 unit 文件、命令行参数、Caddyfile 或日志。

## 启动顺序

1. 将三个角色的主配置和独立敏感 TOML 放到受信目录；生产 Worker 必须启用
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
现有 Auction-BF PostgreSQL v28 使用独立的备份格式、严格表/投影预检和显式接管命令，不能交给
普通 `migrate` 或当前 Rust backup/restore 猜测处理；完整停机、备份、确认、接管与失败恢复流程见
[`LEGACY_V28_ADOPTION.md`](LEGACY_V28_ADOPTION.md)。
旧版 retention 工具本身只允许 inventory 并明确禁止 mutation；当前 Rust 已保持该边界，未来
完整 retention policy 和 mutation 需要独立设计与授权，不属于旧版功能迁移。

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
并在 `pg_dump` 前后保持同一迁移版本。恢复必须由操作员先停止外部进程并准备同名空库；
`postgres-restore` 不会终止连接、清理现有 schema 或破坏性重建数据库。需要由仓库工具准备空库时，
必须单独显式运行 `postgres-rebuild --mode empty-for-restore`，完整确认和回执流程见
[`RECOVERY.md`](RECOVERY.md)。

旧版 v28 的 `postgres-legacy-v28-backup`、离线 verify 和 adoption 使用另一种 manifest。该格式
故意不被 `postgres-backup-verify`/`postgres-restore` 接受；接管恢复点不能误当成已经带 SQLx
历史的当前 Rust 备份。

SQLite 只用于本地或单主机 Lite 部署：数据库文件必须位于本地文件系统，每个角色至多一
个实例，Worker 并发为一。不能用本目录的 PostgreSQL 备份流程把 SQLite 当作生产高可用
替代品。

反向代理示例见 [`caddy/Caddyfile`](caddy/Caddyfile)。
systemd 服务模板见 [`systemd/README.md`](systemd/README.md)；模板必须先替换运行账号和安装路径。
三个角色 unit 不自动迁移；只有显式启动总控时执行 forward-only migration，任何 unit 都不会自动
备份、恢复、DROP/CREATE 数据库或执行 retention 删除。
