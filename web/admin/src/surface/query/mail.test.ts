// 邮件入站 query：rekey 类型往返与未知类型清除。

import { expect, test } from "bun:test";

import { mailQueryToIntakeQuery, useMailQueryState } from "./mail";
import { mountQueryState } from "./query-fixture";

test("rekey 筛选在地址栏和列表查询之间往返", () => {
  const mounted = mountQueryState("/admin/mail?kind=rekey", useMailQueryState);
  try {
    const query = mounted.state.state.peek();
    expect(mailQueryToIntakeQuery(query).kind).toBe("rekey");
    mounted.state.set(query, "push");
    expect(mounted.fake.writes.map(({ mode, url }) => [mode, [...url.searchParams]])).toEqual([
      ["push", [["kind", "rekey"]]],
    ]);
  } finally {
    mounted.dispose();
  }
});

test("未知类型从列表筛选中移除", () => {
  const mounted = mountQueryState("/admin/mail?kind=rekey_unknown", useMailQueryState);
  try {
    const query = mounted.state.state.peek();
    expect(mailQueryToIntakeQuery(query).kind).toBe("");
    mounted.state.set(query, "push");
    expect(mounted.fake.location.search).toBe("");
  } finally {
    mounted.dispose();
  }
});
