# auction-process

[文档导航](../../docs/README.md) · [架构](../../docs/architecture.md) · [契约](../../docs/contracts/README.md)

共享操作系统信号、有界 drain 和 Tokio 任务所有权原语。

## 本页导航

- [源码入口](#源码入口)
- [职责与实现边界](#职责与实现边界)
- [资源所有权](#资源所有权)
- [平台与第二次信号](#平台与第二次信号)
- [维护与验证](#维护与验证)

## 源码入口

[src/lifecycle/mod.rs](src/lifecycle/mod.rs) 定义 `TerminationSignals`、`DrainOutcome`、`wait_for_drain` 与 `TaskGuard`。

## 职责与实现边界

只提供进程生命周期原语，不读取配置、打开监听器、发布心跳或决定业务停止顺序；这些职责由各装配根持有。

## 资源所有权

默认 drain 上限为 10 秒。超时或二次信号会丢弃 drain future，但具体监听器和资源仍须由调用方关闭。
`TaskGuard` 在尚未 join 时自动 abort，避免普通 `JoinHandle` 被丢弃后留下脱离所有权的任务。

## 平台与第二次信号

受支持的 Linux x86_64 原生程序同时订阅 SIGINT/SIGTERM；五个 app 装配根实施平台限制。
此底层原语保留的非 Unix Ctrl-C 分支不构成项目平台支持承诺。订阅失败应阻断启动；
第二次信号要求立即强制收口，不为剩余任务重新延长总预算。

## 维护与验证

```bash
cargo check --locked -p auction-process
```

仅静态编译；默认检查与禁止默认运行的测试范围见[开发指南](../../docs/development.md)，
运行证据见[验证状态](../../docs/status.md)。修改公共行为时同步对应契约。
