// 玩家支持 query：恰好一个检索入口且参考编号必须是规范 UUID。

import { expect, test } from "bun:test";
import { mountQueryState } from "./query-fixture";
import { type SupportQuery, supportQueryValid, useSupportQueryState } from "./support";

const DEFAULT_QUERY: SupportQuery = { user: "", reference: "" };
const REFERENCE = "0b5f1c2e-8a7d-4c3b-9e1f-2a3b4c5d6e7f";

test("恰好一个入口可通过地址栏往返", () => {
  const applied: readonly SupportQuery[] = [
    { user: "alice", reference: "" },
    { user: "Player-One", reference: "" },
    { user: "", reference: REFERENCE },
  ];
  for (const query of applied) {
    expect(supportQueryValid(query)).toBe(true);
    const mounted = mountQueryState("/admin/support", useSupportQueryState);
    try {
      mounted.state.set(query, "push");
      const expectedPairs: [string, string][] =
        query.user === "" ? [["reference", query.reference]] : [["user", query.user]];
      expect([...mounted.fake.location.searchParams]).toEqual(expectedPairs);
      const restored = mountQueryState(
        `/admin/support${mounted.fake.location.search}`,
        useSupportQueryState,
      );
      try {
        expect(restored.state.state.peek()).toEqual(query);
      } finally {
        restored.dispose();
      }
    } finally {
      mounted.dispose();
    }
  }
  const defaults = mountQueryState("/admin/support?user=alice", useSupportQueryState);
  try {
    defaults.state.set(DEFAULT_QUERY, "push");
    expect(defaults.fake.location.search).toBe("");
  } finally {
    defaults.dispose();
  }
});

test("畸形参考编号与空白用户按未设置处理", () => {
  const mounted = mountQueryState("/admin/support?user=+++&reference=REF-1", useSupportQueryState);
  try {
    const parsed = mounted.state.state.peek();
    expect(parsed).toEqual(DEFAULT_QUERY);
    expect(supportQueryValid(parsed)).toBe(false);
  } finally {
    mounted.dispose();
  }
  const trimmed = mountQueryState("/admin/support?user=+bob+", useSupportQueryState);
  try {
    expect(trimmed.state.state.peek()).toEqual({ user: "bob", reference: "" });
  } finally {
    trimmed.dispose();
  }
});

test("两个入口同时设置或同时为空时在请求前拒绝", () => {
  expect(supportQueryValid({ user: "alice", reference: REFERENCE })).toBe(false);
  expect(supportQueryValid(DEFAULT_QUERY)).toBe(false);
  expect(supportQueryValid({ user: "", reference: "not-a-uuid" })).toBe(false);
});
