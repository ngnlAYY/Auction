// 服务端时钟校准采信范围的回归断言。

import { expect, test } from "bun:test";

const { responseDateIsCurrent } = await import("./clock");

test("only_responses_that_cannot_be_replayed_calibrate_the_server_clock：只有不可能被重放的响应可以校准时钟", () => {
  // 会话、身份化读取与命令响应：任何缓存都不得保存，两类请求都采信。
  for (const cacheControl of ["no-store", "private, No-Store"]) {
    expect(responseDateIsCurrent(cacheControl, false), cacheControl).toBe(true);
    expect(responseDateIsCurrent(cacheControl, true), cacheControl).toBe(true);
  }
  // 进行中场次目录：共享缓存不得保存，但浏览器可在新鲜期内重放，只在再验证请求上采信。
  const catalog = "private, max-age=2, stale-while-revalidate=5";
  expect(responseDateIsCurrent(catalog, true)).toBe(true);
  expect(responseDateIsCurrent(catalog, false)).toBe(false);
  // 可被浏览器或 CDN 重放的响应带着生成时的 Date；再验证也可能由 CDN 的旧副本应答。
  for (const replayable of [
    "public, max-age=300, s-maxage=300, stale-while-revalidate=3600",
    "public, max-age=86400, s-maxage=86400, stale-while-revalidate=604800",
    "public, max-age=5",
    "no-cache",
    "max-age=30",
    "",
  ]) {
    expect(responseDateIsCurrent(replayable, false), replayable).toBe(false);
    expect(responseDateIsCurrent(replayable, true), replayable).toBe(false);
  }
  // 没有缓存头的响应无法判断来源。
  expect(responseDateIsCurrent(null, false)).toBe(false);
  expect(responseDateIsCurrent(null, true)).toBe(false);
});
