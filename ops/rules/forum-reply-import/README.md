# 论坛回帖规则导入稿（D6）

本目录是 M1 的玩家可见说明更新稿，对应 [docs/guide/forum.md](../../../docs/guide/forum.md)
中“按导入时刻判定”的现行语义。五个文件是 Admin“规则内容”的五语言编辑稿；
已有规则内容时，必须由用户在 Admin 逐语言编辑并发布，不能直接用 CLI 覆盖。

`auction-cli rules import` 只用于首次发布或补齐缺失语言；任一语言已存在不同正文时，
命令会整体拒绝（见 [apps/auction-cli/README.md#规则导入](../../../apps/auction-cli/README.md#规则导入)）。只有目标环境
尚无对应语言内容时，才使用以下命令：

```bash
auction-cli rules import --directory ops/rules/forum-reply-import \
  --command-id <稳定UUID> --actor <操作者>
```

`en.md`、`zh-CN.md`、`zh-TW.md`、`ja.md`、`ko.md` 的标题与正文必须保持同一语义；
发布前按 Admin 规则校验和玩家指南复核。该目录只含文档，不改变程序、数据库或默认配置。
