# 主机加固基线

[ops/README.md](README.md) · [ops/config/README.md](config/README.md) · [ops/postgres/README.md](postgres/README.md) · [ops/deploy/RECOVERY.md](deploy/RECOVERY.md)

systemd 资源上限与沙箱、SSH、防火墙、PostgreSQL 监听、无人值守升级与重启窗口、时间同步、journald 与 PostgreSQL 小版本升级。模板随发布包提供，主机配置由用户执行；仓库不安装 unit、不改主机服务、不代替现场核对。线上实际 unit 名可能是 `auction-test-*`，须把相同指令合并进实际启用的 unit 文件后 `systemctl daemon-reload`，再按 `service restart` 受控生效。

### systemd 资源上限与沙箱

所有 `*.service.example` 都声明资源上限与 `LimitCORE=0`；三个角色、合并部署与四个 oneshot 另加沙箱选项。上限是失控保护，不是容量规划：模板按约 1.9 GB 内存的单机取值（部署前用 `free -m` 核实主机容量），正常占用远低于上限。

| unit | MemoryMax | TasksMax | 沙箱 |
| --- | ---: | ---: | --- |
| `auction-public-api` | 512M | 256 | 完整 |
| `auction-admin-api` | 512M | 256 | 完整 |
| `auction-worker` | 768M | 512 | 完整 |
| `auction-server`（合并部署） | 1200M | 1024 | 完整 |
| `auction-backup`、`auction-base-backup`、`auction-wal-upload` | 768M | 256 | 完整 |
| `auction-alert-report` | 256M | 128 | 完整 |
| `auction-stack` | 512M | 128 | 保守 |

“完整”沙箱 = `CapabilityBoundingSet=`（空）、`PrivateDevices=true`、`ProtectClock=true`、`ProtectHostname=true`、`ProtectKernelLogs=true`、`ProtectProc=invisible`、`ProcSubset=pid`、`RestrictNamespaces=true`、`RestrictRealtime=true`、`SystemCallArchitectures=native`、`SystemCallFilter=@system-service`、`SystemCallErrorNumber=EPERM`、`MemoryDenyWriteExecute=true`，并保留模板原有的 `ProtectSystem=strict`、`ProtectHome=true`、`NoNewPrivileges=true`、`RestrictAddressFamilies=` 等。服务只读写已列出的 `ReadWritePaths`，沙箱不改变启动顺序、就绪判定与停止预算。`auction-stack` 以 root 运行 `auction-cli service` 并调用 `systemctl`，只加不改变编排能力的选项（不加 `CapabilityBoundingSet=`、`SystemCallFilter=` 与 `ProtectProc=`）。

实测暴露评分（systemd 257.13，`systemd-analyze security --offline=true` 分析仓库模板）：三个角色、`auction-server` 与四个 oneshot 均为 1.5 OK；`auction-stack` 为 5.2 MEDIUM。评分只反映模板声明，不代表主机上实际加载的 unit 未被本地修改；改动模板后应重跑该命令并记录输出。`auction-stack` 的剩余风险来自以 root 调用 `systemctl`，属于编排能力的必要放宽。

线上核对（在目标主机，只读）：

```bash
systemd-analyze security auction-test-worker.service
systemctl show auction-test-worker.service \
  -p MemoryMax -p MemoryCurrent -p MemoryPeak -p TasksMax -p TasksCurrent -p LimitCORE
systemctl show auction-test-worker.service \
  -p CapabilityBoundingSet -p ProtectProc -p ProcSubset -p MemoryDenyWriteExecute -p SystemCallFilter
pid=$(systemctl show -p MainPID --value auction-test-worker.service)
grep -E '^(Seccomp|CapEff):' /proc/$pid/status
```

`Seccomp: 2` 表示过滤器已装载，`CapEff: 0000000000000000` 表示没有有效 capability。`MemoryCurrent` 长期超过 `MemoryMax` 的 70% 或 `TasksCurrent` 接近 `TasksMax` 时，先查真实原因（泄漏、池大小、并行 CLI）再调大上限，不能把 OOM kill 当正常回收。角色启动失败且 journal 出现 `EPERM` 时按顺序放宽：先删 `MemoryDenyWriteExecute=`，再删 `SystemCallFilter=` 与 `SystemCallErrorNumber=`，最后才放宽资源上限；每次只改一项，`daemon-reload` 后受控重启。

### SSH

只允许密钥登录，禁止 root 直接登录：

```
PermitRootLogin no
PasswordAuthentication no
KbdInteractiveAuthentication no
PubkeyAuthentication yes
AllowUsers <运维账号>
```

改完先 `sshd -t` 校验，再 `systemctl reload ssh`（不要断开现有会话），另开终端确认能登录。核对：`sshd -T | grep -E '^(permitrootlogin|passwordauthentication|kbdinteractiveauthentication|pubkeyauthentication|allowusers)'`。需要 root 权限时用 `sudo -i` 或 `su -`；始终保留至少一个可登录账号与一把离线备份密钥。

### 账号、密钥文件与 sudo

运行账号只用于服务，不做交互登录；迁移 owner、维护与 R2 凭据只放在 owner-only 文件。逐项核对权限与 sudo 面：

```bash
find /etc/auction -maxdepth 2 -perm /007 -ls        # 期望仅 /run 之类已声明目录，配置不应 group/other 可写
stat -c '%a %U:%G %n' /etc/auction/secrets.toml /etc/auction/owner/postgres.toml \
  /var/lib/auction/deployment /var/backups/auction /var/lib/auction/backup
ls -l /etc/sudoers.d/ && sudo -l -U auction
```

期望 `secrets.toml` 0600、owner/维护凭据 0600、部署状态与备份目录 0700；`auction` 不需要任何 sudo 权限。核对 systemd 是否被本地 drop-in 放宽：

```bash
systemctl cat auction-test-worker.service | grep -E 'MemoryMax|TasksMax|CapabilityBoundingSet|SystemCallFilter|ProtectSystem'
```

若出现与模板不一致的覆盖，按模板修正后再 `daemon-reload` 并按[ops/README.md#升级](README.md#升级)的受控流程重启。不要用 `chmod -R` 批量放宽 `/etc/auction`。

### 防火墙与端口

入站默认拒绝，只开放 SSH 与反向代理端口。Public/Admin 默认走 Unix socket、PostgreSQL 只监听本机，因此除 SSH 外通常只需 80/443（ACME 与 HTTPS）；经 Cloudflare 代理时可进一步只允许 Cloudflare 地址段访问 443。

```bash
ss -lntup        # 核对实际监听；0.0.0.0/:: 上不属于预期的端口都要处理
nft list ruleset
```

最小 nftables 示例（按实际端口与来源调整，先 `nft -c -f /etc/nftables.conf` 试运行）：

```
table inet filter {
  chain input {
    type filter hook input priority 0; policy drop;
    ct state established,related accept
    iif lo accept
    tcp dport 22 accept
    tcp dport { 80, 443 } accept
    ip protocol icmp accept
    ip6 nexthdr icmpv6 accept
  }
  chain forward { type filter hook forward priority 0; policy drop; }
  chain output  { type filter hook output priority 0; policy accept; }
}
```

非本仓库服务在公网监听的端口不属于本基线，须改为只监听本机或在防火墙显式拒绝。

### PostgreSQL 只监听本机

`listen_addresses` 只含 `127.0.0.1`（或只用 Unix socket），`pg_hba.conf` 不对非本机来源开放 `trust`。配置片段与连接预算见 [ops/postgres/README.md](postgres/README.md)。核对：

```bash
ss -lntp | grep 5432                    # 期望只有 127.0.0.1:5432
psql -Atc "SHOW listen_addresses"
```

### 无人值守升级与重启窗口

安全更新可以自动安装，但**自动重启必须避开 Final 与保护窗口**：CLI 的停机门禁只能拦 `service stop/restart`，拦不住主机重启，也不能把 stack 的 ExecStop 当关机屏障。

```
# /etc/apt/apt.conf.d/20auto-upgrades
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
# /etc/apt/apt.conf.d/50unattended-upgrades
Unattended-Upgrade::Automatic-Reboot "false";
```

选择一：保持 `Automatic-Reboot "false"`，由运维在保护窗口外 `auction-cli service stop`、`reboot`、开机后核对 `service status`。选择二：把 `Automatic-Reboot-Time` 设到确认无 Final 的固定时段，并接受重启仍可能撞上临时场次；两者都要写进维护排期。核对：

```bash
apt-config dump | grep -E 'APT::Periodic::Unattended-Upgrade|Unattended-Upgrade::Automatic-Reboot'
systemctl list-timers 'apt-daily*' --all
test -f /var/run/reboot-required && cat /var/run/reboot-required /var/run/reboot-required.pkgs
```

`needrestart` 可能在升级后自动重启 PostgreSQL 或 sshd；确认 `/etc/needrestart/conf.d/` 未启用自动重启模式（`$nrconf{restart} = 'l'` 为列表模式），否则小版本升级会绕过时段门禁。

### 时间同步

出价截止、会话期限、代理到期与证据时间窗都依赖本机时钟。启用 systemd-timesyncd 或 chrony 并确认已同步：

```bash
timedatectl                      # 期望 System clock synchronized: yes、NTP service: active
timedatectl show -p NTPSynchronized --value
chronyc tracking                 # 使用 chrony 时
```

时区保持 UTC 或与日志/审计一致，不在运行中手工 `date -s`。

### journald 持久化与容量

JSON 日志是排障与审计证据来源，必须持久化并设上限（根分区余量有限，WAL 暂存与备份也在同一磁盘）。`/etc/systemd/journald.conf` 的 `[Journal]` 至少设置 `Storage=persistent`、`SystemMaxUse=2G`、`SystemMaxFileSize=64M`、`RuntimeMaxUse=256M`、`RateLimitIntervalSec=30s`、`RateLimitBurst=10000`。`RateLimitBurst` 调低会丢弃高频 WARN，排障前先看是否有限流记录。核对与维护：

```bash
journalctl --disk-usage
journalctl --verify
systemctl restart systemd-journald      # 只影响日志服务，不重启角色
journalctl --vacuum-size=1G             # 需要腾空间时；先导出要保留的证据
```

### PostgreSQL 小版本升级

小版本升级（18.x → 18.y）二进制兼容，不需要 `pg_upgrade`，但会重启数据库并断开全部连接，因此必须当作计划停机并纳入[停机时段保护](README.md#停机时段保护)：

1. 选在无 Final、且保护窗口覆盖整个升级时长的时段。
2. `auction-cli --config-dir /etc/auction service stop`；被拒绝就改期，确需强制时按 `--force --reason` 规则记录原因。
3. `auction-cli backup create --database auction --output /var/backups/auction/auction-<日期>.dump` 并 `backup verify`。
4. `apt-get update && apt-get install --only-upgrade postgresql-18`（不跨大版本，不手工 `pg_upgrade`），然后 `systemctl restart postgresql`。
5. 核对 `pg_isready`、`psql -Atc "SELECT version()"`、`SHOW shared_preload_libraries`（`pg_stat_statements` 仍加载）与迁移版本未变。
6. `auction-cli --config-dir /etc/auction service start --owner-secrets /etc/auction/owner/postgres.toml`，再 `service status` 确认三角色就绪。

跨大版本升级不属于本节，须走 `pg_upgrade` 与完整恢复演练。仍未落地的项：`auction-stack` 完整沙箱、出站 `IPAddressDeny=`/`IPAddressAllow=` 白名单（依赖稳定的外部端点清单）、`PrivateUsers=true`（会破坏 Unix socket 属主与 peer 凭据校验），以及配置管理与漂移检测。
