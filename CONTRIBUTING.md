# 参与开发

协作规范的唯一来源是 [AGENTS.md](AGENTS.md)：模块职责、领域不变量、配置与迁移规则、文档维护都在那里。本页只给出最短上手路径。

1. 按[docs/development.md](docs/development.md)准备工具链与本地 PostgreSQL。
2. 修改前阅读相关模块的 README 与[docs/architecture.md](docs/architecture.md)，确认改动所属的 owner。
3. 提交前运行：

   ```bash
   just verify            # 策略门禁 + fmt + check + feature + clippy + wasm
   just test              # 行为变更：纯逻辑、单元与本地替身
   just test-db           # 涉及数据库：本机 PG18 临时集群，可用 AUCTION_TEST_DATABASE_URL 复用已有服务器
   just release-check     # 需要完整发布制品时（Fat LTO，较慢）
   ```

约定：

- 提交标题使用 `type(scope): summary`，正文说明保护的不变量与验证范围。
- 路由、事件、DTO、配置、迁移变化必须同步对应文档；文档使用中文，Markdown 链接相对当前文档目录，正文展示的仓库路径以仓库根为起点。
- 迁移只追加，已发布的迁移文件不能修改。
- 正式发布只在 `main` 上打 `v<workspace version>` tag 触发 CI；部署只接受该 tag 产生的 GitHub Release 资产，本地构建或分支候选 artifact 不可部署。
- 不提交凭据、私有 TOML、`.keyenv`、机器专属路径或构建产物。
- `just verify` 保持静态门禁；行为变更随功能维护并运行受影响范围的测试。已批准的测试类别、数据库隔离与尚未接入时的覆盖限制见[docs/development.md#测试](docs/development.md#测试)；其他行为验证先说明范围与环境并取得授权。
- 安全问题不要开公开 issue，请直接联系维护者；修复发布前不公开复现细节。
