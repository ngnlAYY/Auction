# auction-process

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

三个服务共享的信号、有界 drain 和 abort-on-drop 任务原语。

## 本页导航

- [源码入口](#源码入口)
- [职责与实现边界](#职责与实现边界)
- [资源所有权](#资源所有权)
- [平台与第二次信号](#平台与第二次信号)
- [维护与验证](#维护与验证)

## 源码入口

| 入口 | 职责 |
| --- | --- |
| [src/lifecycle](src/lifecycle) | 信号、drain 结果和 TaskGuard |

## 职责与实现边界

三个可部署进程共用的操作系统生命周期原语。

## 资源所有权

该 crate 只负责订阅 SIGINT/SIGTERM、区分首次与二次终止信号、执行默认 10 秒有界 drain，
以及用 abort-on-drop 守卫防止 Tokio 任务在强制收口时脱离。它不读取配置、不打开监听器、
不发布心跳，也不决定任何业务任务的停止顺序；这些所有权仍属于 `apps/public-api`、
`apps/admin-api` 与 `apps/worker` 装配根。

## 平台与第二次信号

非 Unix 平台使用 Tokio 的 Ctrl-C 抽象。Unix 平台显式订阅 SIGINT 与 SIGTERM，使本地终止和
systemd 默认停止信号进入同一条路径；drain 期间第二次信号会要求装配根立即强制收口。

## 维护与验证

修改本模块时同步对应[契约](../../docs/contracts/README.md)与源码入口；跨模块理由按[决策记录规则](../../docs/documentation.md)维护。

```bash
cargo check --locked -p auction-process
```

该命令只做静态编译；完整门禁见[开发指南](../../docs/development.md)，运行证据边界见[验证状态](../../docs/status.md)。
