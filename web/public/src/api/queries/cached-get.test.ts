// 公开只读 GET 非 2xx 分类的回归断言：只有繁忙与网关状态可以回退旧快照。

import { expect, test } from "bun:test";

const { failureOutcome } = await import("./cached-get");

test("only_busy_and_gateway_statuses_are_transient_rejections：只有繁忙与网关状态是暂时性拒绝", () => {
  for (const status of [429, 502, 503, 504, 520, 521, 522, 523, 524]) {
    expect(failureOutcome<null, string>(status, "busy"), String(status)).toEqual({
      kind: "busy",
      error: "busy",
    });
  }
  // 其余非 2xx（含无 ETag 却收到的 304、500、CDN 的 525/526 证书错误与业务拒绝）仍失败关闭。
  for (const status of [304, 400, 401, 403, 404, 409, 410, 422, 500, 501, 525, 526]) {
    expect(failureOutcome<null, string>(status, "rejected"), String(status)).toEqual({
      kind: "rejected",
      error: "rejected",
    });
  }
});
