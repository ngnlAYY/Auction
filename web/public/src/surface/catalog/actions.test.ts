// 目录行操作栏的详情入口与代理快照可用性判定。

import { expect, test } from "bun:test";

const { detailEntryAction, proxySnapshotUsable } = await import("./actions");

test("detail_entry_keeps_proxy_management_reachable_for_leading_bidders：领先者仍可进入代理页签管理授权", () => {
  // 普通出价可用：进直接出价，代理由弹窗页签切换。
  expect(detailEntryAction(false, true, false)).toBe("bid");
  // 回归：领先者的出价入口被禁用后，仍要能进入代理页签降低已有授权。
  expect(detailEntryAction(true, true, true)).toBe("proxy");
  // 达到金额上限的非领先者没有可执行的操作；拍品不可竞价时一律禁用。
  expect(detailEntryAction(true, true, false)).toBeNull();
  expect(detailEntryAction(true, false, true)).toBeNull();
  expect(detailEntryAction(true, false, false)).toBeNull();
});

test("finished_read_is_always_usable：读取完成的结果总是可用", () => {
  // 闭合状态的占位值在读取完成后同样可用：此时没有代理视图依赖它的内容。
  expect(proxySnapshotUsable(true, false, false, true)).toBe(true);
  expect(proxySnapshotUsable(true, true, true, true)).toBe(true);
  // 读取失败也要交给表单，由它展示错误与重试入口。
  expect(proxySnapshotUsable(true, true, true, false)).toBe(true);
});

test("refresh_keeps_last_fetched_snapshot_while_view_is_open：刷新时保留视图打开期间的最后快照", () => {
  // 回归：竞价事件触发的刷新不能卸载正在填写的代理表单。
  expect(proxySnapshotUsable(false, true, true, true)).toBe(true);
});

test("first_read_never_reuses_placeholder：首次读取前不复用占位值", () => {
  // 打开代理视图后的首次读取完成前只有占位值，必须保持加载而不是显示创建表单。
  expect(proxySnapshotUsable(false, false, true, true)).toBe(false);
});

test("refresh_drops_snapshot_when_view_closed_or_last_read_failed：视图关闭或上次读取失败时不沿用快照", () => {
  expect(proxySnapshotUsable(false, true, false, true)).toBe(false);
  expect(proxySnapshotUsable(false, true, true, false)).toBe(false);
});
