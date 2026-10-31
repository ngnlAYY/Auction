# auction-server

[文档导航](../../docs/README.md) · [运维手册](../../ops/README.md) · [首次设置](../../crates/auction-setup/README.md)

单文件合并部署：一个可执行文件、一个 systemd unit，在本机以子进程监管 Worker、Admin、Public 三个角色。适合单主机、小规模部署；分体部署（三个 unit + `auction-cli service` 编排）见[运维手册](../../ops/README.md)。两种方式不能同时用于同一套配置。

需要 `postgres` 与 `web-embed` feature（发布脚本的 single 变体：`just build-single`）。

## 用法

```bash
auction-server --config-dir /var/lib/auction/config
```

| 参数 | 默认 | 说明 |
| --- | --- | --- |
| `--config-dir` | `auction-config` | 三角色配置目录；不存在时进入首次设置，已有目录绝不覆盖 |
| `--setup-listen` | `127.0.0.1:8091` | 首次设置页监听地址 |
| `--allow-remote-setup` | 关 | 允许经受信 HTTPS 代理访问首次设置；须同时给出下面两项 |
| `--setup-origin` | — | 代理对外的精确 HTTPS origin（不含路径） |
| `--setup-proxy-ip` | — | 唯一受信代理的源地址 |
| `--startup-timeout-seconds` | 60 | 每个角色等待就绪的秒数（1–300） |

`auction-server` 不接受 `AUCTION_CONFIG_FILE`。隐藏参数 `--role`、`--config-file` 只供它派生子进程使用。

## 启动流程

1. 规范化配置目录，计算同级运行目录 `.<目录名>.server`（如 `/var/lib/auction/.config.server`，仅 owner 可访问），对其中的 `owner.lock` 加独占锁——同一配置目录只能有一个 supervisor（`auction-cli` 也通过该锁拒绝操作合并部署的数据库）。
2. 配置目录不存在：运行首次设置页（表单须确认“初始化并启动”），生成合并布局配置。
3. 加载三角色配置并预检：只支持 PostgreSQL；必须启用 Worker control 且允许当前 UID；三角色共用数据库目标、`common_file` 与 `runtime_config_file`，装备数据密钥与 `bot_players` 快照一致；Admin 的 `worker_config_file` 指向本部署的 Worker 配置；listener 不重叠、不使用动态端口，socket 路径未被占用。
4. 首次设置后：在 300 秒内把专用空库初始化为当前 schema（`initialize_empty_postgres` 确认库为空且无其他客户端）。
5. 只读检查数据库就绪（30 秒）：schema 不是当前版本时拒绝启动，提示停服后以同一服务账号执行 `auction-cli --config-dir <配置目录> db migrate`。普通启动从不隐式迁移。
6. 按 Worker → Admin → Public 顺序派生子进程（独立进程组、父进程死亡时子进程收到 SIGTERM、继承 owner 锁），每个角色在 `--startup-timeout-seconds` 内通过就绪探针（Worker 为 control socket 的 `/v1/health/ready`，Admin/Public 为 `/health/ready`）后再启动下一个。全部就绪后输出 `auction-server ready: worker, admin, public`。

## 运行与退出

- 任一子进程意外退出，supervisor 立即停止其余角色并以非零状态退出——不在进程内自动重启。
- 收到 SIGTERM/SIGINT 时按 Public → Admin → Worker 反序发送 SIGTERM 并等待，总预算 135 秒（每角色 10 秒排空 + tracing 刷新）；超时或第二次信号强制结束。

## systemd

示例 [`ops/deploy/systemd/auction-server.service.example`](../../ops/deploy/systemd/auction-server.service.example)：`StateDirectory=auction` 承载配置目录与运行目录，`KillMode=mixed`（信号只发给 supervisor），`TimeoutStopSec=155s`，`Restart=no`——首次初始化或迁移失败需要人工核对，不用自动重启掩盖。

## 日常运维

合并部署同样使用 `auction-cli`，但 `service` 子命令只适用于分体部署。数据库迁移、建库、恢复与重建必须先停止 `auction-server`，再以同一服务账号执行（`auction-cli` 通过 `owner.lock` 取得独占停机证明）；备份创建可在运行时执行，离线备份校验也不要求停服。详见 [auction-cli](../auction-cli/README.md)。
