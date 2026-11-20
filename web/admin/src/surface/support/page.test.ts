// 支持检索表单与历史恢复共用 UTF-8 字节上限。

import { expect, jest, test } from "bun:test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { byId, input, okReply, submit, withPage } from "../browser-page-fixture.test";
import { settle } from "../solid-dom-fixture";
import { onlyElement } from "../t10-view-fixture.test";

const LIMIT = `${"漢".repeat(85)}a`;
const TOO_LONG = [`${LIMIT}b`, "漢".repeat(86)] as const;
const REFERENCE = "0b5f1c2e-8a7d-4c3b-9e1f-2a3b4c5d6e7f";
const BASE = "/player-support";
const ERROR = "请输入不超过 256 个 UTF-8 字节的内容。";

async function loadPage() {
  return (
    await loadRenderModule<Pick<typeof import("./page"), "page">>(
      `${import.meta.dir}/page.tsx`,
      "page",
    )
  ).page;
}

test("支持检索拒绝 257 字节和 86 个汉字，保留草稿，修短到 256 字节后可提交", async () => {
  const page = await loadPage();
  await withPage(
    BASE,
    page,
    () =>
      okReply({
        bids: [],
        deals: [],
        deliveries: [],
        notifications: [],
        obligations: [],
        references: [],
        truncated: false,
        user: null,
      }),
    async (view) => {
      const user = byId(view.document, "support-user");
      const form = onlyElement(view.document, "form");
      for (const oversized of TOO_LONG) {
        await input(user, oversized);
        jest.useFakeTimers();
        try {
          await submit(form);
          jest.advanceTimersByTime(0);
          await settle();
        } finally {
          jest.useRealTimers();
        }
        expect(user.value).toBe(oversized);
        expect(user.getAttribute("aria-invalid")).toBe("true");
        expect(user.getAttribute("aria-describedby")).toBe("support-user-error");
        expect(byId(view.document, "support-user-error").textContent).toBe(ERROR);
        expect(view.document.activeElement).toBe(user);
        expect(view.address()).toEqual([]);
        expect(view.requests).toHaveLength(0);
      }

      await input(user, ` ${LIMIT} `);
      expect(user.getAttribute("aria-invalid")).toBe("false");
      expect(view.document.getElementById("support-user-error")).toBeNull();
      await submit(form);
      expect(view.address()).toEqual([["user", LIMIT]]);
      expect(
        view
          .takeRequests()
          .map(({ method, url }) => [
            method,
            url.pathname,
            url.searchParams.get("user"),
            url.searchParams.get("reference"),
          ]),
      ).toEqual([["GET", "/api/v1/admin/player-support", LIMIT, null]]);
    },
  );
});

test("超长支持书签与前进后退保留检索词且不查询，合法参考编号单独查询", async () => {
  const page = await loadPage();
  const oversized = TOO_LONG[1];
  const bookmark = new URLSearchParams({ user: oversized, reference: REFERENCE });
  await withPage(
    `${BASE}?${bookmark}`,
    page,
    () =>
      okReply({
        bids: [],
        deals: [],
        deliveries: [],
        notifications: [],
        obligations: [],
        references: [],
        truncated: false,
        user: null,
      }),
    async (view) => {
      const assertRejected = (user: string, reference: string) => {
        expect(byId(view.document, "support-user").value).toBe(user);
        expect(byId(view.document, "support-reference").value).toBe(reference);
        expect(byId(view.document, "support-user-error").textContent).toBe(ERROR);
        expect(view.takeRequests()).toEqual([]);
      };
      assertRejected(oversized, REFERENCE);
      const user = byId(view.document, "support-user");
      await input(user, "");
      await submit(onlyElement(view.document, "form"));
      expect(view.address()).toEqual([["reference", REFERENCE]]);
      expect(
        view
          .takeRequests()
          .map(({ method, url }) => [
            method,
            url.pathname,
            url.searchParams.get("user"),
            url.searchParams.get("reference"),
          ]),
      ).toEqual([["GET", "/api/v1/admin/player-support", null, REFERENCE]]);
      await view.back();
      assertRejected(oversized, REFERENCE);
      await view.forward();
      expect(view.document.getElementById("support-user-error")).toBeNull();
      expect(view.takeRequests().map(({ url }) => url.searchParams.get("reference"))).toEqual([
        REFERENCE,
      ]);

      const otherOversized = `${LIMIT}b`;
      await view.restore(new URLSearchParams({ user: otherOversized }));
      assertRejected(otherOversized, "");
      await view.back();
      expect(view.takeRequests().map(({ url }) => url.searchParams.get("reference"))).toEqual([
        REFERENCE,
      ]);
      await view.forward();
      assertRejected(otherOversized, "");
    },
  );
});
