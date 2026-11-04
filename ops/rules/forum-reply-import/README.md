# 论坛回帖规则导入稿（D6）

本目录是 M1 的玩家可见说明更新稿，对应 [论坛同步与回帖竞价](../../../docs/guide/forum.md)
中“按导入时刻判定”的现行语义。五个文件可直接作为
`auction-cli rules import --directory` 的输入，由用户核对后发布到 Admin“规则内容”：

```bash
auction-cli rules import --directory ops/rules/forum-reply-import \
  --command-id <稳定UUID> --actor <操作者>
```

`en.md`、`zh-CN.md`、`zh-TW.md`、`ja.md`、`ko.md` 的标题与正文必须保持同一语义；
发布前按 Admin 规则校验和玩家指南复核。该目录只含文档，不改变程序、数据库或默认配置。
