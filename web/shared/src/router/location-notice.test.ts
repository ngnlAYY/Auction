// 共享路由的地址通知及规范化契约。

import { expect, test } from "bun:test";

import { writeQueryPairs } from "../net/query";
import { openRouter } from "./test-window";

test("查询 replace 成功后立即刷新地址快照", () => {
  const opened = openRouter("/?world=main");
  try {
    expect(opened.router.currentLocationString()).toBe("/?world=main");
    writeQueryPairs(["auction"], [["auction", "auction-uuid"]], "replace");
    expect(opened.router.currentLocationString()).toBe("/?world=main&auction=auction-uuid");
    expect(opened.fake.location.search).toBe("?world=main&auction=auction-uuid");
  } finally {
    opened.disposeRouter();
    opened.restore();
  }
});

test("查询 push 同步地址快照并复用当前路由匹配", () => {
  const opened = openRouter("/");
  try {
    const match = opened.router.current();
    writeQueryPairs(["auction"], [["auction", "auction-uuid"]], "push");
    expect(opened.router.currentLocationString()).toBe("/?auction=auction-uuid");
    expect(opened.fake.location.search).toBe("?auction=auction-uuid");
    expect(opened.router.current()).toBe(match);
  } finally {
    opened.disposeRouter();
    opened.restore();
  }
});

test("历史条目标识随 push 和前进后退切换，replace 保留标识", () => {
  const { router, fake, disposeRouter, restore } = openRouter("/");
  try {
    const original = router.currentEntryKey();
    const match = router.current();
    writeQueryPairs(["page"], [["page", "2"]], "push");
    const pushed = router.currentEntryKey();
    expect(pushed).not.toBe(original);
    expect(router.current()).toBe(match);

    writeQueryPairs(["page"], [], "replace");
    expect(router.currentLocationString()).toBe("/");
    expect(router.currentEntryKey()).toBe(pushed);

    fake.history.back();
    expect(router.currentLocationString()).toBe("/");
    expect(router.currentEntryKey()).toBe(original);
    fake.history.forward();
    expect(router.currentEntryKey()).toBe(pushed);
  } finally {
    disposeRouter();
    restore();
  }
});

test("路由销毁后通知不再生效", () => {
  const opened = openRouter("/");
  try {
    writeQueryPairs(["world"], [["world", "main"]], "replace");
    expect(opened.router.currentLocationString()).toBe("/?world=main");
    opened.disposeRouter();
    writeQueryPairs(["world"], [["world", "isekai"]], "replace");
    expect(opened.fake.location.search).toBe("?world=isekai");
    expect(opened.router.currentLocationString()).toBe("/?world=main");
  } finally {
    opened.disposeRouter();
    opened.restore();
  }
});

test("冷启动与站内导航规范化尾斜杠，保留查询和片段", () => {
  const opened = openRouter("/rules/?lang=ja#schedule");
  try {
    expect(opened.router.currentLocationString()).toBe("/rules?lang=ja#schedule");
    opened.router.push("/me/?bid_page=2#account-bids-title");
    expect(opened.router.currentLocationString()).toBe("/me?bid_page=2#account-bids-title");
    opened.router.replace("/rules/?lang=en#schedule");
    expect(opened.router.currentLocationString()).toBe("/rules?lang=en#schedule");
  } finally {
    opened.disposeRouter();
    opened.restore();
  }
});

test("根地址的第二个问号属于参数名，规范化不删参数", () => {
  const opened = openRouter("/??page=2&ref=a%26b#results");
  try {
    expect(opened.router.currentLocationString()).toBe("/??page=2&ref=a%26b#results");
  } finally {
    opened.disposeRouter();
    opened.restore();
  }
});

test("保留动态路径段里的编码字符", () => {
  const opened = openRouter("/");
  try {
    opened.router.push("/lots/a%2Fb%25/?ref=x#info");
    expect(opened.router.currentLocationString()).toBe("/lots/a%2Fb%25?ref=x#info");
    expect(opened.router.current()).toMatchObject({ kind: "matched", params: { code: "a/b%" } });
  } finally {
    opened.disposeRouter();
    opened.restore();
  }
});
