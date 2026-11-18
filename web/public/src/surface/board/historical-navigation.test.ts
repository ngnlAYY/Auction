// 显式场次的世界纠偏、生产外壳交付与近期结算目录再验证。

import { expect, test } from "bun:test";
import { EQUIPMENT_CATEGORY_CODES } from "@auction/contracts/dto/public/catalog/lot";
import { t } from "@auction/web-shared/ui/i18n";
import {
  AUCTION_ID,
  dispatch,
  END_AT,
  hasClass,
  mountCompetition,
  renderedText,
  settle,
  waitFor,
} from "../commands/competition-fixture.test";
import { reject, success } from "../shell/session-fixture";

const OPEN_AT = "2025-11-15T08:00:00Z";

test("生产外壳等到 world 写入成功才发布场次与筛选目录", async () => {
  const query = `auction=${AUCTION_ID}&%74ype=%63lo&keep=%2b%20x`;
  const addressesAtCatalogRead: string[] = [];
  const page = await mountCompetition("isekai", false, {
    href: `/?world=main&${query}#lots`,
    rejectHistoryWrites: true,
    readDirectory(url) {
      if (url.searchParams.has("category")) {
        addressesAtCatalogRead.push(window.location.search + window.location.hash);
      }
      return null;
    },
  });
  const board = () => page.find((element) => hasClass(element, "public-auction-page"));
  const tickerTitle = () =>
    renderedText(page.find((element) => hasClass(element, "console-ticker__auction")));
  const activeWorld = () =>
    renderedText(
      page.find(
        (element) =>
          hasClass(element, "public-auction-filter__tab") &&
          element.getAttribute("aria-pressed") === "true",
      ),
    );
  const lotReads = () =>
    page.reads
      .filter((path) => path.startsWith(`/api/v1/auctions/${AUCTION_ID}/lots?`))
      .map((path) => new URL(path, page.location));
  const categoryReads = () => lotReads().filter((url) => url.searchParams.has("category"));
  const title = t("ticker-auction-title", { number: 12 });
  try {
    await waitFor(() => page.reads.some((url) => url.includes("/current?world=isekai")));
    await settle();
    expect(page.location.search).toBe(`?world=main&${query}`);
    expect(categoryReads()).toHaveLength(0);
    expect(board()).toBeNull();
    expect(tickerTitle()).toBe(t("nav-auctions"));
    expect(activeWorld()).toBe(t("auction-world-main"));
    expect(
      renderedText(page.find((element) => hasClass(element, "console-ticker__world"))),
    ).toContain(t("auction-world-main"));
    const feedback = page.need(
      (element) =>
        element.getAttribute("role") === "alert" &&
        renderedText(element).includes(t("world-correction-error")),
    );

    const retry = page.need((element) => hasClass(element, "ui-async-state__retry"), feedback);
    dispatch(retry, "Click");
    expect(page.location.search).toBe(`?world=main&${query}`);
    expect(addressesAtCatalogRead).toEqual([]);
    page.rejectWrites(false);
    dispatch(retry, "Click");
    await waitFor(
      () => board() !== null && tickerTitle() === title && categoryReads().length === 1,
    );
    expect(page.location.search).toBe(`?${query}&world=isekai`);
    expect(page.location.hash).toBe("#lots");
    expect(activeWorld()).toBe(t("auction-world-isekai"));
    expect(
      renderedText(page.find((element) => hasClass(element, "console-ticker__world"))),
    ).toContain(t("auction-world-isekai"));
    expect(board()?.getAttribute("aria-label")).toBe(t("auction-header-title", { number: 12 }));
    expect(categoryReads().map((url) => url.searchParams.get("category"))).toEqual(["clo"]);
    expect(addressesAtCatalogRead).toEqual([`?${query}&world=isekai#lots`]);
  } finally {
    page.dispose();
  }
});

test.each(["listed", "revalidated", "missing", "unavailable"] as const)(
  "已结算深链只在正常目录缺席时再验证近期结算（%s）",
  async (status) => {
    const page = await mountCompetition("main", true);
    const headers: (string | null)[] = [];
    const path = "/api/v1/auctions?page=1&page_size=200&status=settled";
    const summary = {
      id: AUCTION_ID,
      world_id: "main",
      sequence: 12,
      status: "settled",
      scheduled_open_at: OPEN_AT,
      settled_at: END_AT,
      lot_count: 0,
      sold_count: 0,
      unsold_count: 0,
      sold_amount: 0,
      state_version: 1,
    };
    try {
      page.server.readDirectory = (url) =>
        success({
          auction_id: AUCTION_ID,
          auction_status: "settled",
          projection_kind: "live",
          presentation_mode: "live",
          bidding_open: false,
          opens_at: OPEN_AT,
          preview_revision: 0,
          lots: [],
          total: 0,
          page: 1,
          page_size: Number(url.searchParams.get("page_size")),
          category_counts: {
            all: 0,
            ...Object.fromEntries(EQUIPMENT_CATEGORY_CODES.map(([key]) => [key, 0])),
          },
        });
      page.server.readRounds = (url, init) => {
        if (url.pathname + url.search !== path) return null;
        headers.push(new Headers(init?.headers).get("If-None-Match"));
        if (headers.length === 2) {
          if (status === "missing") return new Response(null, { status: 304 });
          if (status === "unavailable") return reject("storage_unavailable", 503);
        }
        const listed = status === "listed" || (status === "revalidated" && headers.length === 2);
        return Response.json(
          {
            code: "ok",
            message: "ok",
            data: {
              auctions: listed ? [summary] : [],
              page: 1,
              page_size: 200,
              total: listed ? 1 : 0,
            },
          },
          { headers: { ETag: '"recent-settled"' } },
        );
      };
      const target = `/?auction=${AUCTION_ID}&world=main`;
      page.go(target);
      const board = () => page.find((element) => hasClass(element, "public-auction-page"));
      const error = () => page.find((element) => hasClass(element, "ui-async-state--error"));
      await waitFor(() => board() !== null || error() !== null);
      expect(headers).toEqual(status === "listed" ? [null] : [null, '"recent-settled"']);
      expect(page.reads.filter((url) => url === path)).toHaveLength(status === "listed" ? 1 : 2);
      const hasRound = status === "listed" || status === "revalidated";
      expect(board() !== null).toBe(hasRound);
      expect(error() !== null).toBe(!hasRound);
      expect(page.location.pathname + page.location.search).toBe(target);
    } finally {
      page.dispose();
    }
  },
);
