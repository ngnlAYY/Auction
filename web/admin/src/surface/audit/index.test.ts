// 审计页的历史草稿、精确时间边界与读失败重试。

import { expect, spyOn, test } from "bun:test";
import { adminAuditFilterAbsoluteTime } from "../../api/audit";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { withUtc } from "../auctions/auction-time-fixture.test";
import {
  byId,
  click,
  input,
  inputAndExpectValueUpdate,
  type MountedPage,
  okReply,
  type PageRequest,
  pendingRead,
  STORAGE_UNAVAILABLE,
  selectedOptions,
  submit,
  textParts,
  withPage,
} from "../browser-page-fixture.test";
import { type TestDocument, valueUpdates } from "../solid-dom-fixture";
import { auditTimeInput } from "./filter-state";

const AUDIT = "/api/v1/admin/audit";

function emptyPage({ method, url }: PageRequest) {
  expect([method, url.pathname]).toEqual(["GET", AUDIT]);
  const page = Number(url.searchParams.get("page"));
  const page_size = Number(url.searchParams.get("page_size"));
  return okReply({
    records: [],
    page,
    page_size,
    total: 0,
    has_more: false,
    next_cursor: null,
    next_cursor_token: "",
  });
}

async function withAudit(
  query: string,
  check: (page: MountedPage) => Promise<void> | void,
  respond: (
    request: PageRequest,
  ) => ReturnType<typeof emptyPage> | Promise<ReturnType<typeof emptyPage>> = pendingRead,
): Promise<void> {
  const { page } = await loadRenderModule<Pick<typeof import("."), "page">>(
    new URL("./index.tsx", import.meta.url).pathname,
    "page",
  );
  await withPage(`/audit?${query}`, page, respond, check);
}

function auditForm(document: TestDocument) {
  const forms = document.querySelectorAll("form");
  expect(forms).toHaveLength(1);
  const form = forms[0];
  if (form === undefined) {
    throw new Error("缺少审计筛选表单");
  }
  expect(typeof form.attributes.onSubmit).toBe("function");
  return form;
}

function oneGet(page: MountedPage): URLSearchParams {
  const requests = page.takeRequests();
  expect(requests.map(({ method, url }) => [method, url.pathname])).toEqual([["GET", AUDIT]]);
  const first = requests[0];
  if (first === undefined) {
    throw new Error("缺少审计 GET");
  }
  expect(first.url.pathname + first.url.search).toStartWith(`${AUDIT}?`);
  return first.url.searchParams;
}

test("audit_retries_failed_read_after_two_page_renders", async () => {
  let reads = 0;
  await withAudit(
    "",
    async (page) => {
      const title = byId(page.document, "admin-audit-title");
      const failed = "所需服务暂时不可用。请稍后再试。";
      const empty = "没有符合条件的审计记录。";
      expect(textParts(page.document.roots)).toContain(failed);
      expect(textParts(page.document.roots)).not.toContain(empty);
      await page.translateIn("en");
      expect(title.textContent).toBe("Audit log");
      await page.translateIn("zh-CN");
      expect(byId(page.document, "admin-audit-title")).toBe(title);
      expect(title.textContent).toBe("审计日志");
      expect([...oneGet(page)]).toEqual([
        ["page", "1"],
        ["page_size", "10"],
      ]);
      await click(page.document, "重试");
      expect([...oneGet(page)]).toEqual([
        ["page", "1"],
        ["page_size", "10"],
      ]);
      expect(textParts(page.document.roots)).toContain(empty);
      expect(textParts(page.document.roots)).not.toContain(failed);
    },
    (request) => {
      reads += 1;
      if (reads === 1) {
        return STORAGE_UNAVAILABLE;
      }
      expect(reads).toBe(2);
      expect(request.url.searchParams.get("page")).toBe("1");
      return emptyPage(request);
    },
  );
});

test("audit_bookmark_selects_the_options_used_by_the_first_get", async () => {
  await withAudit("actor_type=worker&source=cli&result=failure", (page) => {
    const query = oneGet(page);
    for (const [key, id, value] of [
      ["actor_type", "audit-actor-type", "worker"],
      ["source", "audit-source", "cli"],
      ["result", "audit-result", "failure"],
    ] as const) {
      expect(query.get(key)).toBe(value);
      expect(selectedOptions(page.document, id)).toEqual([value]);
    }
  });
});

test("restored_audit_actor_shows_every_original_and_waits_for_submission", async () => {
  for (const [originals, escaped] of [
    [
      ["", "alice"],
      ['""', '"alice"'],
    ],
    [["alice\n"], ['"alice\\n"']],
  ] as const) {
    await withAudit("actor=first", async (page) => {
      const form = auditForm(page.document);
      expect(oneGet(page).get("actor")).toBe("first");
      const invalidAddress = new URLSearchParams(originals.map((value) => ["actor", value]));
      await page.restore(invalidAddress);
      expect(page.takeRequests()).toEqual([]);
      const actor = byId(page.document, "audit-actor");
      expect(actor.getAttribute("aria-describedby")).toBe("audit-actor-error");
      const error = byId(page.document, "audit-actor-error");
      expect(error.querySelectorAll("code").map((node) => node.textContent)).toEqual([...escaped]);
      await submit(form);
      expect(page.address()).toEqual([...invalidAddress]);
      expect(page.takeRequests()).toEqual([]);
      await input(actor, "fixed");
      expect(page.address()).toEqual([...invalidAddress]);
      expect(page.takeRequests()).toEqual([]);
      await submit(form);
      expect(oneGet(page).get("actor")).toBe("fixed");
    });
  }
});

test("draft_tracks_history_filter_changes_and_preserves_edits_on_page_and_sort_changes", async () => {
  await withAudit("actor=first", async (page) => {
    const actor = byId(page.document, "audit-actor");
    const form = auditForm(page.document);
    expect(actor.value).toBe("first");
    expect(oneGet(page).get("actor")).toBe("first");
    await inputAndExpectValueUpdate(actor, "unsaved");
    const draftWrites = valueUpdates(actor);
    for (const query of ["actor=first", "actor=first&sort=actor", "actor=first&page=2"]) {
      await page.restore(query);
      expect(byId(page.document, "audit-actor")).toBe(actor);
      expect(actor.value).toBe("unsaved");
      expect(valueUpdates(actor)).toEqual(draftWrites);
    }
    expect(oneGet(page).get("page")).toBe("2");

    for (const expected of ["second", "first", "second"]) {
      const before = valueUpdates(actor).length;
      await page.restore(`actor=${expected}`);
      expect(byId(page.document, "audit-actor")).toBe(actor);
      expect(actor.value).toBe(expected);
      expect(valueUpdates(actor).slice(before)).toContain(expected);
      expect(oneGet(page).get("actor")).toBe(expected);
    }

    // 另走一次分页后的提交，让上面的历史切换直接覆盖未提交草稿。
    await inputAndExpectValueUpdate(actor, "unsaved");
    await page.restore("actor=second&page=2");
    const paged = oneGet(page);
    expect([paged.get("actor"), paged.get("page")]).toEqual(["second", "2"]);
    await submit(form);
    expect(oneGet(page).get("actor")).toBe("unsaved");
  });
});

test("returning_from_a_sort_preserves_the_draft_after_applying_a_padded_actor", async () => {
  const auditQuery = await import("../query/audit");
  const observeQuery = spyOn(auditQuery, "useAuditQueryState");
  try {
    await withAudit("actor=first", async (page) => {
      const mounted = observeQuery.mock.results[0];
      if (mounted?.type !== "return") {
        throw new Error("审计页未挂载查询状态");
      }
      const { state } = mounted.value;
      const actor = byId(page.document, "audit-actor");
      const form = auditForm(page.document);
      expect(actor.value).toBe("first");
      page.takeRequests();
      await inputAndExpectValueUpdate(actor, " alice ");
      await submit(form);
      expect(oneGet(page).get("actor")).toBe("alice");
      const sorted = new URLSearchParams(page.address());
      sorted.set("sort", "actor");
      await inputAndExpectValueUpdate(actor, "unsaved");
      const draftWrites = valueUpdates(actor);
      await page.restore(sorted);
      expect(state.peek().sort).toBe("actor");
      expect(actor.value).toBe("unsaved");
      expect(valueUpdates(actor)).toEqual(draftWrites);

      await page.back();
      expect(observeQuery).toHaveBeenCalledTimes(1);
      const restored = state.peek();
      expect(restored.sort).toBe("");
      expect(auditQuery.auditQueryPageRequest(restored)?.[1].actor).toBe("alice");
      expect(byId(page.document, "audit-actor")).toBe(actor);
      expect(actor.value).toBe("unsaved");
      expect(page.address()).toContainEqual(["actor", "alice"]);
      expect(valueUpdates(actor)).toEqual(draftWrites);

      await submit(form);
      expect(oneGet(page).get("actor")).toBe("unsaved");
    });
  } finally {
    observeQuery.mockRestore();
  }
});

test("prefill_displays_browser_local_minutes_and_keeps_the_exact_time_on_submit", () =>
  withUtc(() => {
    const bound = "2026-11-18T09:05:37.500123+08:00";
    expect(auditTimeInput(bound)).toBe("2026-11-18T01:05");
    expect(adminAuditFilterAbsoluteTime(bound)).toBe("2026-11-18T01:05:37.500123Z");
    expect(auditTimeInput("2026-11-18T17:25")).toBe("2026-11-18T17:25");
    expect(auditTimeInput("")).toBe("");
  }));

test("audit_keeps_its_prefilled_bound_when_correcting_the_other_field", () =>
  withUtc(() => {
    const bound = "2026-11-18T01:05:37.500123456Z";
    return withAudit(`from=${bound}`, async (page) => {
      const form = auditForm(page.document);
      const to = byId(page.document, "audit-to");
      expect(byId(page.document, "audit-from").value).toBe("2026-11-18T01:05");
      expect(oneGet(page).get("occurred_from")).toBe(bound);
      await input(to, "2026-11-31T09:05");
      expect(byId(page.document, "audit-to")).toBe(to);
      expect(to.getAttribute("aria-invalid")).toBe("false");
      expect(to.getAttribute("aria-describedby")).toBeNull();
      expect(page.document.getElementById("audit-to-error")).toBeNull();
      expect(page.takeRequests()).toEqual([]);

      await submit(form);
      expect(byId(page.document, "audit-to")).toBe(to);
      expect(to.getAttribute("aria-invalid")).toBe("true");
      expect(to.getAttribute("aria-describedby")).toBe("audit-to-error");
      expect(textParts(byId(page.document, "audit-to-error"))).toContain(
        "请输入有效的浏览器本地日期和时间。",
      );
      expect(page.takeRequests()).toEqual([]);

      await input(to, "2026-11-20T09:05");
      await submit(form);
      const query = oneGet(page);
      expect(query.get("occurred_from")).toBe(bound);
      expect(query.get("occurred_to")).toBe("2026-11-20T09:05:00Z");
    });
  }));
