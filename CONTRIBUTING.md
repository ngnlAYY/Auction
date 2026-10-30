# 参与开发

协作规范的唯一来源是 [AGENTS.md](AGENTS.md)：模块职责、领域不变量、配置与迁移规则、文档维护都在那里。本页只给出最短上手路径。

1. 按[开发指南](docs/development.md)准备工具链与本地 PostgreSQL。
2. 修改前阅读相关模块的 README 与[架构](docs/architecture.md)，确认改动所属的 owner。
3. 提交前运行：

   ```bash
   just verify            # 策略门禁 + fmt + check + feature + clippy + wasm
   just release-check     # 需要完整发布制品时（Fat LTO，较慢）
   ```

约定：

- 提交标题使用 `type(scope): summary`，正文说明保护的不变量与验证范围。
- 路由、事件、DTO、配置、迁移变化必须同步对应文档；文档使用中文，链接与仓库路径使用以仓库根为起点的相对路径（不写 `./`、`../`）。
- 迁移只追加，已发布的迁移文件不能修改。
- 不提交凭据、私有 TOML、`.keyenv`、机器专属路径或构建产物。
- 默认只做静态验证，不新增或执行行为测试；如需行为验证，先说明范围与环境。
- 安全问题不要开公开 issue，请直接联系维护者；修复发布前不公开复现细节。
