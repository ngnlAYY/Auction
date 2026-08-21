# auction-application

应用层：用例、窄端口和事务编排。

- `ports.rs`：`AuctionRepository`、`Clock` 等端口。使用 Rust 原生 async trait 返回类型，
  不依赖 `async-trait` 宏。
- `service.rs`：用例实现，例如 `AuctionService::create`。
- `error.rs`：应用错误，保留领域错误分类。

依赖只指向 `auction-domain`。适配器在外层实现这里声明的端口。

**不得**读取进程配置、打开网络监听、直接执行 SQL 或直接调用外部服务。跨模块协作必须
通过窄端口，不共享大结构体绕过边界。

每个可变命令都要有稳定的 `command_id`，重复提交返回原结果；同一 ID 搭配不同负载必须报
冲突。竞价写入的事务顺序见 `docs/architecture.md` 的「竞价写入不变量」。
