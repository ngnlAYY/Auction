# 参与开发

协作约束的唯一来源是 [AGENTS.md](AGENTS.md)：模块 owner、领域不变量、配置与迁移规则、文档维护都在那里。
本页只给最短上手路径。

```bash
just verify                 # 策略门禁 + fmt + check + feature + lint + wasm
just release-check          # 需要完整制品时（Fat LTO，较慢）
```

- 提交前工作树须自洽；正式发布要求干净工作树。
- 提交标题用 `type(scope): summary`，正文说明保护的不变量与验证范围。
- 文档、路由、配置、迁移变化必须同步对应 owner 文档；链接写成仓库根相对路径。
- 不提交凭据、私有 TOML、机器专属路径或构建产物。
- 默认只做静态验证，不新增或执行行为测试；范围与成本见 AGENTS.md。
- 安全问题不要开公开 issue，直接联系维护者，修复发布前不公开复现细节。
