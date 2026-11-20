// T1 接口与表单测试的浏览器请求构造和内存回执。

import { expect, spyOn, test } from "bun:test";

import { type FetchHandler, fixtureFetch } from "./api-request-fixture";

/** 局部替换 fetch，并提供 Bun 类型要求的预连接入口以捕获意外调用。 */
export function spyOnBrowserFetch() {
  const spy = spyOn(globalThis, "fetch");
  return {
    respondWith(handler: FetchHandler): void {
      spy.mockImplementation(fixtureFetch(handler));
    },
    restore(): void {
      spy.mockRestore();
    },
  };
}

test("请求替身返回内存回执，释放后恢复原 fetch", async () => {
  const original = globalThis.fetch;
  const fixture = spyOnBrowserFetch();
  try {
    fixture.respondWith((input) => {
      expect(input).toBe("/test-fixture");
      return Promise.resolve(new Response("fixture reply"));
    });
    expect(await (await fetch("/test-fixture")).text()).toBe("fixture reply");
  } finally {
    fixture.restore();
  }
  expect(globalThis.fetch).toBe(original);
});
