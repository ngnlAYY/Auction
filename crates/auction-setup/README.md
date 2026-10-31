# auction-setup

[文档导航](../../docs/README.md) · [首次部署](../../ops/README.md#首次部署)

PostgreSQL 首次部署的一次性设置服务：在没有任何配置时提供 Web 设置页（复用 Admin 浏览器资源的设置入口），收集数据库连接、Admin 凭据、Public/Admin origin 与端口、周排期与起拍价等，校验后原子生成三角色配置目录。需要 `postgres` feature；实际设置页还要求入口内嵌 Admin Web 资源（`web-embed`）。

## 流程

1. 启动时目标配置目录必须不存在，绑定监听（默认 `127.0.0.1:8091`），并在 stderr 打印设置页地址与一次性设置令牌。
2. 浏览器以 `x-setup-token` 头携带令牌调用 `POST /api/v1/setup/verify`、`/validate`、`/complete`；令牌常量时间比较，三个接口共用一个非阻塞操作锁，并发请求返回 `setup_busy`。请求体上限 8 KiB，单请求 10 秒超时。
3. `validate` 与 `complete` 校验表单并在 8 秒内试连预先建立的数据库；这里只验证连通性，连接池随即关闭，不证明库为空或 schema 已就绪。`complete` 调用 `auction-config` 的 `FirstRunConfig` 以示例为模板生成配置：非敏感文件 `0640`、敏感文件 `0600`，先在同级私有临时目录写好并经三角色 loader 校验，再以 `renameat2(RENAME_NOREPLACE)` 原子发布，已存在目录绝不覆盖。
4. 生成的配置启用 Worker control（组与 UID 由入口提供；Admin 默认组为 `auction-worker-control`、UID 为运行账号，合并入口使用运行账号的组与 UID）、信任本机代理 `127.0.0.1/32`、论坛公开地址取 Public origin；不生成 `.keyenv`、三组数据密钥或外部 Cookie，这些需在设置完成后补充。
5. 发布结果无法确认（目录可能已出现）时进入 `OutcomeUnknown`，禁止继续，须人工检查。

## 两种使用方式

| 入口 | 行为 |
| --- | --- |
| `auction-admin-api`（无参数且找不到配置）或 `auction-admin-api setup --output-dir <绝对路径>` | 分体部署：写完配置后退出，不建库、不迁移 |
| `auction-server --config-dir <目录>`（目录不存在） | 合并部署：表单须确认“初始化并启动”，配置写为合并布局（Worker control socket 位于同级运行目录），随后由 `auction-server` 初始化专用空库并启动三角色 |

默认只监听 loopback，推荐经 SSH 隧道访问。远程访问须显式提供精确的 HTTPS origin 与唯一受信代理 IP（`auction-server --allow-remote-setup --setup-origin … --setup-proxy-ip …`），只有来自该代理且证明为 HTTPS 的请求才被接受；完成后应立即撤除临时代理站点。
