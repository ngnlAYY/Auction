# auction-setup

[文档导航](docs/README.md) · [初始化契约](docs/contracts/routes.md#首次部署初始化) · [实现与验证状态](docs/status.md)

PostgreSQL 单机首次部署的独立 HTTP 服务：一次性 `/setup` 页面、令牌校验与配置保存。配置生成归
[auction-config](crates/auction-config/README.md)、连通性检查归 [auction-storage](crates/auction-storage/README.md)；
本 API 不迁移，保存成功也不代表服务就绪。

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/server](crates/auction-setup/src/server) | 首次部署路由、请求状态与终态 |
| [src/server/access.rs](crates/auction-setup/src/server/access.rs) | Host/Origin、代理信任与请求预算 |
| [src/server/lifecycle.rs](crates/auction-setup/src/server/lifecycle.rs) | 连接所有权、信号与有界退出 |

## 请求与安全边界

只有目标配置目录明确不存在才启动，已有或损坏配置不降级为设置页。默认仅 loopback，远程合并设置必须显式配置
精确 HTTPS origin 与受信代理 IP，不信任任意转发头；正式 Admin 不注册设置接口。verify、validate、complete 每次都
校验 `x-setup-token` 后取得同一非阻塞操作锁，竞争返回 `setup_busy`；verify 无请求体、不消费令牌。全部响应附加
`no-store` 与共享安全头，请求头预算 5 秒、正文与异步处理合计 10 秒（表单与 DTO 见[初始化契约](docs/contracts/routes.md#首次部署初始化)）。

## 配置发布与退出

数据库五字段、正式服务端口和每周排期交给配置 owner 校验及物化：端口仅生成回环监听，HTTP origin 支持
`localhost`、`127.0.0.1` 与规范 `[::1]`，非回环仍要求 HTTPS；排期候选即使关闭也校验，
默认值以[配置模板](ops/config/README.md)为准。保存后拒绝新请求；发布后持久化确认失败进入不可逆的
`setup_outcome_unknown`，保留目录并等待操作者停止核对，不能重新验证令牌后重试。终态和完成通知由同一操作锁拥有，
从同步发布到终态及退出通知之间没有 await，也不再单独获取通知锁。请求取消不能证明目录未写入，异步 deadline
不能抢占阻塞磁盘调用；不能简单改成可脱离请求继续执行的后台写入。分体完成后退出，再由受保护的 CLI 迁移和启动；
合并 complete 要求 `confirm_initialize=true`，`run_first_run_with_policy` 只有 `Completed` 可让装配根继续空库初始化，
`Interrupted`/`OutcomeUnknown` 不可继续。
停止接收后最多排空 10 秒，二次信号或超时强制取消连接并核对终态。

## 维护与验证

```bash
cargo check --locked -p auction-setup --features postgres
```

仅静态编译；默认检查与禁止默认运行的测试范围见[开发指南](docs/guide/development.md)，运行证据见[验证状态](docs/status.md)。
