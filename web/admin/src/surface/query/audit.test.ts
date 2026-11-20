// 审计 query：提交与恢复共用一套 URL 身份，时间链接规范化后仍表示同一时刻。

import { expect, test } from "bun:test";
import { writeLocationPreservingScroll } from "@auction/web-shared/router";
import { type AdminAuditFilter, adminAuditFilterDefault } from "../../api/audit";
import { parseStrictRfc3339 } from "../../api/session-time";
import { loadRenderModule } from "../../solid-render-fixture.test";
import type { AuditField } from "../audit/filter-state";
import { byId, okReply, withPage } from "../browser-page-fixture.test";
import {
  type AuditQuery,
  auditQueryDefault,
  auditQueryPageRequest,
  auditQueryWithFilter,
  useAuditQueryState,
} from "./audit";
import { mountQueryState } from "./query-fixture";

function withTimeZone<T>(timeZone: string, run: () => T): T {
  const originalTimeZone = process.env.TZ;
  process.env.TZ = timeZone;
  try {
    return run();
  } finally {
    if (originalTimeZone === undefined) {
      delete process.env.TZ;
    } else {
      process.env.TZ = originalTimeZone;
    }
  }
}

function encode(pairs: readonly (readonly [string, string])[]): string {
  return new URLSearchParams(pairs.map(([key, value]) => [key, value])).toString();
}

function filter(overrides: Partial<AdminAuditFilter>): AdminAuditFilter {
  return { ...adminAuditFilterDefault(), ...overrides };
}

const EMPTY_FILTER = { kind: "text", value: "" } as const;
const FILTER_ORDER: Readonly<Record<AuditField, number>> = {
  actor: 0,
  entity: 1,
  action: 2,
  request_id: 3,
  actor_type: 4,
  source: 5,
  result: 6,
  from: 7,
  to: 8,
};
const EMPTY_FILTER_VALUES = Array.from({ length: 9 }, () => EMPTY_FILTER);

function expectInvalidState(query: AuditQuery, field: AuditField, values: readonly string[]): void {
  expect([...query.filter.values]).toEqual(
    EMPTY_FILTER_VALUES.map((value, index) =>
      index === FILTER_ORDER[field] ? { kind: "invalid", values } : value,
    ),
  );
  expect(query.page).toBe(1n);
  expect(query.sort).toBe("");
  expect(query.direction).toBe("asc");
}

test("提交与恢复的文本筛选共享同一 URL 身份", () => {
  const applied = auditQueryWithFilter(
    auditQueryDefault(),
    filter({
      actor: " alice ",
      entity: " auction:sample lot ",
      action: " auction.create ",
      requestId: "  ",
    }),
  );
  const mounted = mountQueryState("/admin/audit", useAuditQueryState);
  let writtenAddress: string;
  try {
    mounted.state.set(applied, "push");
    expect([...mounted.fake.location.searchParams]).toEqual([
      ["actor", "alice"],
      ["entity", "auction:sample lot"],
      ["action", "auction.create"],
    ]);
    writtenAddress = mounted.fake.location.href;
  } finally {
    mounted.dispose();
  }

  const restored = mountQueryState(writtenAddress, useAuditQueryState);
  try {
    const restoredQuery = restored.state.state.peek();
    expect(auditQueryPageRequest(restoredQuery)).not.toBeNull();
    expect(restoredQuery).toEqual(applied);
  } finally {
    restored.dispose();
  }

  const invalid = mountQueryState(
    `/admin/audit?${encode([["actor", " invalid\0actor "]])}`,
    useAuditQueryState,
  );
  try {
    const query = invalid.state.state.peek();
    expectInvalidState(query, "actor", [" invalid\0actor "]);
    expect(auditQueryPageRequest(query)).toBeNull();
    invalid.state.set(query, "push");
    expect(invalid.fake.writes.map(({ mode, url }) => [mode, [...url.searchParams]])).toEqual([
      ["push", [["actor", " invalid\0actor "]]],
    ]);
  } finally {
    invalid.dispose();
  }
});

test("旧本地审计链接改写为绝对时间", () => {
  // Rust 宿主测试的本地时区固定为 UTC；浏览器沿用同一换算规则。
  withTimeZone("UTC", () => {
    const mounted = mountQueryState("/admin/audit?from=2026-11-18T09%3A05", useAuditQueryState);
    try {
      const query = mounted.state.state.peek();
      const request = auditQueryPageRequest(query);
      expect(request?.[1].occurredFrom).toBe("2026-11-18T09:05:00Z");
      mounted.state.set(query, "push");
      expect([...mounted.fake.location.searchParams]).toEqual([["from", "2026-11-18T09:05:00Z"]]);
    } finally {
      mounted.dispose();
    }
  });
});

test("带偏移的审计链接往返且不改变时刻", () => {
  const input = "2026-11-18T09:05:37.500123456+08:00";
  const normalized = "2026-11-18T01:05:37.500123456Z";
  const mounted = mountQueryState(`/admin/audit?${encode([["to", input]])}`, useAuditQueryState);
  let writtenAddress: string;
  let before: ReturnType<typeof mounted.state.state.peek>;
  try {
    const query = mounted.state.state.peek();
    before = query;
    const request = auditQueryPageRequest(query);
    expect(request?.[1].occurredTo).toBe(normalized);
    mounted.state.set(query, "push");
    expect([...mounted.fake.location.searchParams]).toEqual([["to", normalized]]);
    expect(parseStrictRfc3339(request?.[1].occurredTo ?? "")).toBe(1794963937500123456n);
    expect(parseStrictRfc3339(input)).toBe(1794963937500123456n);
    writtenAddress = mounted.fake.location.href;
  } finally {
    mounted.dispose();
  }

  const restored = mountQueryState(writtenAddress, useAuditQueryState);
  try {
    expect(restored.state.state.peek()).toEqual(before);
  } finally {
    restored.dispose();
  }
});

test("畸形时间不会变成无界筛选", () => {
  const value = "2026-11-31T09:05:00+08:00";
  const mounted = mountQueryState(`/admin/audit?${encode([["from", value]])}`, useAuditQueryState);
  try {
    const query = mounted.state.state.peek();
    expectInvalidState(query, "from", [value]);
    expect(auditQueryPageRequest(query)).toBeNull();
    mounted.state.set(query, "push");
    expect(mounted.fake.writes.map(({ mode, url }) => [mode, [...url.searchParams]])).toEqual([
      ["push", [["from", value]]],
    ]);
  } finally {
    mounted.dispose();
  }
});

test("审计恢复保留重复键与控制字符", () => {
  const fields: readonly (readonly [AuditField, string])[] = [
    ["actor", "alice"],
    ["entity", "auction:sample"],
    ["action", "auction.create"],
    ["request_id", "request-1"],
    ["actor_type", "admin"],
    ["source", "admin"],
    ["result", "success"],
    ["from", "2026-11-18T09:00:00Z"],
    ["to", "2026-11-18T10:00:00Z"],
  ];
  for (const [key, valid] of fields) {
    const validMount = mountQueryState(
      `/admin/audit?${encode([[key, valid]])}`,
      useAuditQueryState,
    );
    try {
      expect(auditQueryPageRequest(validMount.state.state.peek())).not.toBeNull();
    } finally {
      validMount.dispose();
    }

    for (const pairs of [
      [
        [key, ""],
        [key, valid],
      ],
      [[key, `${valid}\n`]],
    ] as const) {
      const mounted = mountQueryState(`/admin/audit?${encode(pairs)}`, useAuditQueryState);
      try {
        const query = mounted.state.state.peek();
        expectInvalidState(
          query,
          key,
          pairs.map(([, value]) => value),
        );
        expect(auditQueryPageRequest(query)).toBeNull();
        mounted.state.set(query, "push");
        expect(mounted.fake.writes.map(({ mode, url }) => [mode, [...url.searchParams]])).toEqual([
          ["push", pairs.map(([name, value]): [string, string] => [name, value])],
        ]);
      } finally {
        mounted.dispose();
      }
    }
  }
});

test("已挂载的审计表单随同页导航及历史切换 GET 参数", async () => {
  const { page } = await loadRenderModule<Pick<typeof import("../audit"), "page">>(
    new URL("../audit/index.tsx", import.meta.url).pathname,
    "page",
  );
  await withPage(
    "/audit?actor=alice&page=3&sort=actor&dir=desc&page_size=50",
    page,
    ({ url }) => {
      const page = Number(url.searchParams.get("page"));
      const pageSize = Number(url.searchParams.get("page_size"));
      return okReply({
        records: [],
        page,
        page_size: pageSize,
        total: 200,
        has_more: page * pageSize < 200,
        next_cursor: null,
        next_cursor_token: "",
      });
    },
    async (mounted) => {
      const requests = () =>
        mounted
          .takeRequests()
          .map(({ method, url }) => [method, url.pathname, Object.fromEntries(url.searchParams)]);
      const filtered = [
        ["GET", "/api/v1/admin/audit", { page: "3", page_size: "50", actor: "alice" }],
      ];
      const defaults = [["GET", "/api/v1/admin/audit", { page: "1", page_size: "10" }]];
      const actor = byId(mounted.document, "audit-actor");
      const descending = () =>
        mounted.document.find((node) => node.getAttribute("aria-sort") === "descending");
      expect(requests()).toEqual(filtered);
      expect(actor.value).toBe("alice");
      expect(descending()?.textContent).toContain("操作者");

      expect(writeLocationPreservingScroll("/audit", "push")).toBe(true);
      await mounted.translateIn("zh-CN");
      expect(requests()).toEqual(defaults);
      expect(actor.value).toBe("");
      expect(descending()).toBeNull();
      await mounted.back();
      expect(requests()).toEqual(filtered);
      expect(actor.value).toBe("alice");
      expect(descending()?.textContent).toContain("操作者");
      await mounted.forward();
      expect(requests()).toEqual(defaults);
      expect(actor.value).toBe("");
      expect(descending()).toBeNull();

      expect(writeLocationPreservingScroll("/audit?actor=bob&actor=carol", "replace")).toBe(true);
      await mounted.translateIn("zh-CN");
      expect(requests()).toEqual([]);
      expect(writeLocationPreservingScroll("/audit?actor=dave&page=2", "replace")).toBe(true);
      await mounted.translateIn("zh-CN");
      expect(requests()).toEqual([
        ["GET", "/api/v1/admin/audit", { page: "2", page_size: "10", actor: "dave" }],
      ]);
      expect(actor.value).toBe("dave");
    },
  );
});
