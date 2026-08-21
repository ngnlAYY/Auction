# 持续集成约定

持续集成与本地 `just verify` 使用同一组默认门禁：Rust 格式化、带 `--locked` 的 Cargo
工作区编译检查、Clippy 静态检查和两个浏览器端 crate 的 `wasm32-unknown-unknown` 目标
编译检查。Rust 工具链固定为
`1.98.0` 当前 stable，并用独立 job 验证 SQLx 0.9 对应的 workspace MSRV `1.94.1`；
Tokio 使用官方 `1.51.x` LTS，其他无官方 LTS 的依赖以 `Cargo.lock` 为准。

持续集成不安装 dx CLI，也不构建前端产物：`auction-http` 的内嵌声明使用 `allow_missing`，
缺少产物时仍能通过全部静态门禁，因此 CI 不依赖 npm 下载 esbuild。前端产物的实际构建属于
发布流程，由 `just web-build` 在具备 dx CLI 的环境执行。

默认工作区检查验证最小依赖图；独立 feature 检查分别编译 PostgreSQL、SQLite 和可选
Redis 路径，Clippy 与 MSRV job 使用 `--all-features` 验证组合图。这样默认制品不会仅为
备用后端或关闭的缓存携带驱动，同时每条受支持路径仍进入静态门禁。

除非用户明确要求，暂不编写或运行单元测试、集成测试、功能测试用例、端到端测试和浏览器
测试。已有测试代码保留，但不在默认持续集成流程中扩展或执行。

将来恢复测试时，SQLite 只验证 Lite 能力清单，可直接使用临时文件或内存数据库；
PostgreSQL 使用服务容器或临时实例验证完整能力。SQLite 不需要运行完整后端的所有契约，
也不能代替 PostgreSQL 专属的隔离、行锁和并发验证。
