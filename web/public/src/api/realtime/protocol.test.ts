// 实时 WebSocket 地址：同源、单拍卖，并声明能解码 hello 的 independent_build。

import { expect, test } from "bun:test";

import { websocketUrl } from "./protocol";
import { AUCTION_ID } from "./subscription-fixture";

test("websocket url declares the independent_build capability", () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, "window") ?? null;
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: { location: new URL("https://example.test") },
  });
  try {
    const url = websocketUrl(AUCTION_ID);
    expect(url.ok).toBe(true);
    if (url.ok) {
      expect(url.value).toBe(
        `wss://example.test/api/v1/ws?auction_id=${AUCTION_ID}&independent_build=1`,
      );
    }
  } finally {
    if (original === null) {
      Reflect.deleteProperty(globalThis, "window");
    } else {
      Object.defineProperty(globalThis, "window", original);
    }
  }
});
