// 排期用例在 UTC 下重现 Rust 非 WASM 测试的本地时间语义。

import { expect, test } from "bun:test";

/** 在一个用例内固定 Bun 的本地时区，结束时恢复开始前的环境。 */
export async function withUtc<T>(run: () => T | Promise<T>): Promise<T> {
  const previous = process.env.TZ;
  try {
    process.env.TZ = "UTC";
    return await run();
  } finally {
    if (previous === undefined) {
      delete process.env.TZ;
    } else {
      process.env.TZ = previous;
    }
  }
}

test("排期夹具在异步回调抛错后恢复原时区", async () => {
  const previous = process.env.TZ;
  const failure = new Error("排期测试回调失败");
  await expect(
    withUtc(async () => {
      await Promise.resolve();
      expect(process.env.TZ).toBe("UTC");
      throw failure;
    }),
  ).rejects.toBe(failure);
  expect(process.env.TZ).toBe(previous);
});
