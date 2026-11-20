// 查询写回把同地址策略透传给共享历史层，并返回写入是否成功。

import { expect, test } from "bun:test";
import { createRoot, flush } from "solid-js";

import { writeLocationPreservingScroll } from "../../router/history";

import { installFakeWindow } from "../../router/test-window";
import { useQueryState, writeQueryPairs } from ".";

test("没有浏览器环境时不写入并返回 false", () => {
  expect(writeQueryPairs(["page"], [["page", "2"]], "push")).toBe(false);
});

test("显式同地址写入照常创建历史项，缺省仍跳过", () => {
  const { fake, restore } = installFakeWindow("/admin?page=1");
  try {
    const before = fake.writes.length;
    expect(writeQueryPairs(["page"], [["page", "1"]], "push")).toBe(false);
    expect(fake.writes.length).toBe(before);

    expect(writeQueryPairs(["page"], [["page", "1"]], "push", { sameAddress: "write" })).toBe(true);
    expect(fake.writes.length).toBe(before + 1);
    expect(fake.location.search).toBe("?page=1");

    expect(writeQueryPairs(["page"], [["page", "1"]], "replace", { sameAddress: "write" })).toBe(
      true,
    );
    expect(fake.writes.length).toBe(before + 2);
  } finally {
    restore();
  }
});

test("浏览器拒绝写入时返回 false", () => {
  const { fake, restore } = installFakeWindow("/admin?page=1");
  try {
    fake.rejectWrites("push");
    expect(writeQueryPairs(["page"], [["page", "1"]], "push", { sameAddress: "write" })).toBe(
      false,
    );
  } finally {
    restore();
  }
});

test("同页导航和历史恢复筛选，页内写回、同址、片段与跨页不重复解析", () => {
  const { fake, restore } = installFakeWindow("/audit?actor=alice");
  const parsed: string[] = [];
  const mounted = createRoot((dispose) => ({
    dispose,
    query: useQueryState(
      ["actor"],
      (pairs) => {
        const actor = pairs.find(([key]) => key === "actor")?.[1] ?? "";
        parsed.push(actor);
        return actor;
      },
      (actor) => (actor === "" ? [] : [["actor", actor]]),
    ),
  }));
  try {
    flush();
    expect(mounted.query.state.peek()).toBe("alice");
    mounted.query.push("bob");
    mounted.query.replace("carol");
    expect(fake.writes.map(({ mode, url }) => [mode, url.search])).toEqual([
      ["push", "?actor=bob"],
      ["replace", "?actor=carol"],
    ]);
    expect(writeLocationPreservingScroll("/audit", "push")).toBe(true);
    expect(mounted.query.state.peek()).toBe("");
    fake.history.back();
    expect(mounted.query.state.peek()).toBe("carol");
    fake.history.forward();
    expect(mounted.query.state.peek()).toBe("");

    fake.rejectWrites(true);
    mounted.query.push("dave");
    expect(fake.location.search).toBe("");
    fake.rejectWrites(false);
    expect(writeLocationPreservingScroll("/audit", "push", { sameAddress: "write" })).toBe(true);
    expect(writeLocationPreservingScroll("/audit#filters", "replace")).toBe(true);
    expect(mounted.query.state.peek()).toBe("dave");

    expect(writeLocationPreservingScroll("/users?actor=eve", "push")).toBe(true);
    expect(mounted.query.state.peek()).toBe("dave");
    fake.history.pushState(null, "", "/mails?actor=mallory");
    fake.history.back();
    expect(mounted.query.state.peek()).toBe("dave");
    fake.history.back();
    expect(mounted.query.state.peek()).toBe("");
    expect(parsed).toEqual(["alice", "", "carol", "", ""]);
  } finally {
    mounted.dispose();
    flush();
    restore();
  }
});
