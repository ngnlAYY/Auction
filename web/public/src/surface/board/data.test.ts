// 看板的目录请求门禁和往期切换器经过生产页面及同源请求边界。

import { expect, jest, onTestFinished, test } from "bun:test";
import { EQUIPMENT_CATEGORY_CODES } from "@auction/contracts/dto/public/catalog/lot";
import { t } from "@auction/web-shared/ui/i18n";
import { flush } from "solid-js";
import { catalogLevelFilterDefault } from "../../api/model";
import { prefetchCurrentLots } from "../../api/queries/lots";
import { invalidateAfterCommand, invalidateAll } from "../../api/swr-cache";
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
import { installDialogFocus } from "../shell/dialog-focus-fixture.test";
import { reject, success } from "../shell/session-fixture";

const STORED_AUCTION = new Map([["hv.public.current-auction.main", AUCTION_ID]]);
const OPEN_AT = "2036-01-01T00:00:00Z";
const CURRENT = {
  id: AUCTION_ID,
  world: "main",
  number: 12,
  status: "live",
  opens_at: "2025-11-15T08:00:00Z",
  scheduled_ends_at: END_AT,
  presentation_mode: "live",
  bidding_open: true,
  preview_revision: 0,
  proxy_default_mode: "fast",
};

/** 生成与该区间应有行数一致的摘要投影；解码器会核对页长与 total。 */
function liveLots(url: URL, total: number) {
  const page = Number(url.searchParams.get("page"));
  const pageSize = Number(url.searchParams.get("page_size"));
  const offset = (page - 1) * pageSize;
  const count = Math.max(0, Math.min(pageSize, total - offset));
  return Array.from({ length: count }, (_, index) => {
    const number = offset + index + 1;
    return {
      id: `11111111-0000-4000-8000-${String(number).padStart(12, "0")}`,
      auction_id: AUCTION_ID,
      lot_number: number,
      status: "live",
      start_price: 1_000,
      current_price: 1_000,
      next_bid: 1_000,
      has_leader: false,
      seller_name: "seller",
      state_version: 1,
      effective_end_at: END_AT,
      equipment: {
        name: "Legendary Cotton Robe of Heimdall",
        category: "Cloth",
        equipment_group: "rare",
        level: 450,
        primary_attribute_initials: "IW",
        avg_roll: 70.0,
        game_equipment_url: "https://hentaiverse.org/equip/123456/0123456789",
      },
    };
  });
}

function liveDirectory(url: URL, total = 0): Response {
  return success({
    auction_id: AUCTION_ID,
    auction_status: "live",
    projection_kind: "live",
    presentation_mode: "live",
    bidding_open: true,
    opens_at: CURRENT.opens_at,
    preview_revision: 0,
    lots: liveLots(url, total),
    total,
    page: Number(url.searchParams.get("page")),
    page_size: Number(url.searchParams.get("page_size")),
    category_counts: {
      ...Object.fromEntries(
        EQUIPMENT_CATEGORY_CODES.map(([key]) => [key, key === "clo" ? total : 0]),
      ),
      all: total,
    },
  });
}

function lotReads(reads: readonly string[]): URL[] {
  return reads
    .filter((path) => path.startsWith(`/api/v1/auctions/${AUCTION_ID}/lots?`))
    .map((path) => new URL(path, "https://auction.test"));
}

async function settleTimedReads(): Promise<void> {
  for (let turn = 0; turn < 128; turn += 1) {
    await Promise.resolve();
    flush();
  }
}

test.each(["type=unknown", "type=one,one", "type=clo&type=sta"])(
  "非法分类 %s 保留原值，预取和正文等待明确修正",
  async (filter) => {
    const page = await mountCompetition("main", false, {
      href: `/?${filter}&external=kept`,
      storage: STORED_AUCTION,
    });
    try {
      await settle();
      expect(lotReads(page.reads)).toHaveLength(0);
      const invalid = page.need(
        (element) =>
          element.getAttribute("aria-live") === "polite" &&
          renderedText(element).includes(
            t(filter.includes("&") ? "filter-error-duplicate" : "filter-category-invalid"),
          ),
      );
      for (const value of new URLSearchParams(filter).getAll("type")) {
        expect(renderedText(invalid)).toContain(value);
      }
      expect(page.location.searchParams.getAll("type")).toEqual(
        new URLSearchParams(filter).getAll("type"),
      );
      const clear = page.need(
        (element) =>
          element.tag === "button" && renderedText(element) === t("filter-category-clear"),
      );
      page.rejectWrites("push");
      dispatch(clear, "Click");
      await settle();
      expect(page.location.searchParams.getAll("type")).toEqual(
        new URLSearchParams(filter).getAll("type"),
      );
      expect(lotReads(page.reads)).toHaveLength(0);
      page.rejectWrites(false);
      dispatch(clear, "Click");
      await waitFor(() => lotReads(page.reads).length !== 0);
      expect(page.location.searchParams.has("type")).toBe(false);
      expect(page.location.searchParams.get("external")).toBe("kept");
      expect(lotReads(page.reads).map((url) => url.searchParams.get("category"))).toEqual([null]);
    } finally {
      page.dispose();
    }
  },
);

test.each([
  ["202", "50", "201"],
  ["9223372036854775807", "200", "51"],
  ["201", "50", "201"],
] as const)("目录页码 %s 按每页 %s 条在 GET 前约束为 %s", async (requested, size, expected) => {
  const pagesAtGet: (string | null)[] = [];
  const page = await mountCompetition("main", false, {
    href: `/?page=${requested}&page_size=${size}&type=clo`,
    storage: STORED_AUCTION,
    readDirectory(url) {
      pagesAtGet.push(new URLSearchParams(window.location.search).get("page"));
      return liveDirectory(url, 10_001);
    },
  });
  try {
    await settle();
    await waitFor(() => lotReads(page.reads).length > 0);
    await settle();
    expect(pagesAtGet).toEqual([expected]);
    expect(page.location.searchParams.get("page")).toBe(expected);
    expect(
      lotReads(page.reads).map((url) => [
        url.searchParams.get("page"),
        url.searchParams.get("page_size"),
        url.searchParams.get("category"),
      ]),
    ).toEqual([[expected, size, "clo"]]);
  } finally {
    page.dispose();
  }
});

test("目录越界页码的 replace 被拒时预取与正文均不发 GET", async () => {
  const page = await mountCompetition("main", false, {
    href: "/?page=202&page_size=50&type=clo",
    storage: STORED_AUCTION,
    rejectHistoryWrites: true,
  });
  try {
    await waitFor(
      () => page.find((element) => renderedText(element) === t("pagination-clamp-error")) !== null,
    );
    expect(page.location.searchParams.get("page")).toBe("202");
    expect(lotReads(page.reads)).toEqual([]);
  } finally {
    page.dispose();
  }
});

test("显式场次纠偏只替换世界，目录仍按原分类读取", async () => {
  const query = `auction=${AUCTION_ID}&%74ype=%63lo&keep=%2b%20x&keep=%7e`;
  const page = await mountCompetition("main", false, {
    href: `/?world=isekai&${query}#lots`,
    readDirectory: liveDirectory,
  });
  try {
    await waitFor(() => lotReads(page.reads).some((url) => url.searchParams.has("category")));
    await settle();
    expect(page.location.search).toBe(`?${query}&world=main`);
    expect(page.location.hash).toBe("#lots");
    expect(lotReads(page.reads).map((url) => url.searchParams.get("category"))).toEqual([
      null,
      "clo",
    ]);
  } finally {
    page.dispose();
  }
});

test("目录总数收缩时只纠正页码，保留原始分类和查询片段", async () => {
  const query = "%74ype=%63lo&keep=%2b%20x";
  const page = await mountCompetition("main", false, {
    href: `/?${query}&page=2`,
    readDirectory: (url) => liveDirectory(url, 1),
  });
  try {
    await waitFor(() => lotReads(page.reads).length === 2);
    await settle();
    expect(page.location.search).toBe(`?${query}`);
    expect(
      lotReads(page.reads).map((url) => [
        url.searchParams.get("page"),
        url.searchParams.get("category"),
      ]),
    ).toEqual([
      ["2", "clo"],
      ["1", "clo"],
    ]);
  } finally {
    page.dispose();
  }
});

test.each(["waiting", "late"] as const)("同 URL 的预取 Retry-After 约束 %s 正文", async (order) => {
  const first = Promise.withResolvers<Response>();
  const current = Promise.withResolvers<Response>();
  const sent: number[] = [];
  const page = await mountCompetition("main", false, {
    href: "/",
    storage: STORED_AUCTION,
    readCurrent: () => (order === "late" ? current.promise : null),
    readDirectory(url) {
      sent.push(performance.now());
      return sent.length === 1 ? first.promise : liveDirectory(url);
    },
  });
  try {
    await settle();
    expect(sent).toHaveLength(1);
    if (order === "waiting") {
      expect(page.find((element) => hasClass(element, "public-auction-page"))).not.toBeNull();
    }
    jest.useFakeTimers();
    onTestFinished(() => jest.useRealTimers());
    const failureAt = performance.now();
    first.resolve(
      Response.json(
        { code: "rate_limited", message: "", data: null },
        { status: 429, headers: { "Retry-After": "4" } },
      ),
    );
    await settleTimedReads();
    jest.advanceTimersByTime(1_000);
    current.resolve(success({ auction: CURRENT }));
    await settleTimedReads();
    expect(page.find((element) => hasClass(element, "public-auction-page"))).not.toBeNull();
    jest.advanceTimersByTime(2_999);
    await settleTimedReads();
    expect(sent).toHaveLength(1);
    jest.advanceTimersByTime(1);
    await settleTimedReads();
    expect(sent).toHaveLength(2);
    expect((sent[1] ?? 0) - failureAt).toBe(4_000);
    expect(lotReads(page.reads).map((url) => url.pathname + url.search)).toEqual(
      Array(2).fill(`/api/v1/auctions/${AUCTION_ID}/lots?page=1&page_size=20&fields=summary`),
    );
  } finally {
    page.dispose();
  }
});

test.each([false, true])("隐藏页交付目录前复核旧快照资格（命令失效 %s）", async (command) => {
  const dom = installDialogFocus();
  const page = await mountCompetition();
  const table = () => page.find((element) => hasClass(element, "public-auction-catalog__table"));
  const readError = () =>
    page.find(
      (element) =>
        element.getAttribute("role") === "alert" &&
        renderedText(element).includes(t("common-load-error")),
    );
  try {
    dom.attach(page.find);
    await waitFor(() => table() !== null);
    page.server.readDirectory = () =>
      Response.json(
        { code: "rate_limited", message: "", data: null },
        { status: 429, headers: { "Retry-After": "4" } },
      );
    jest.useFakeTimers();
    onTestFinished(() => jest.useRealTimers());
    dispatch(
      page.need((element) => hasClass(element, "public-connection-refresh")),
      "Click",
    );
    jest.advanceTimersByTime(80);
    await settleTimedReads();
    const lotsPath = `/api/v1/auctions/${AUCTION_ID}/lots?page=1&page_size=20&fields=summary`;
    expect(lotReads(page.reads).map((url) => url.pathname + url.search)).toEqual([
      lotsPath,
      lotsPath,
    ]);
    expect(readError()).toBeNull();

    if (command) invalidateAfterCommand();
    Object.assign(document, { hidden: true });
    document.dispatchEvent(new Event("visibilitychange"));
    jest.advanceTimersByTime(4_000);
    await settleTimedReads();
    expect(lotReads(page.reads).map((url) => url.pathname + url.search)).toEqual([
      lotsPath,
      lotsPath,
    ]);
    expect(readError() !== null).toBe(command);
    expect(table() !== null).toBe(!command);
  } finally {
    page.dispose();
    dom.restore();
  }
});

test.each(["visible", "hidden", "revoked"] as const)(
  "补读等候在途 GET 后按 %s 状态交付目录",
  async (state) => {
    const dom = installDialogFocus();
    const page = await mountCompetition();
    const pending = Promise.withResolvers<Response>();
    const controller = new AbortController();
    const table = () => page.find((element) => hasClass(element, "public-auction-catalog__table"));
    const error = () =>
      page.find(
        (element) =>
          element.getAttribute("role") === "alert" &&
          renderedText(element).includes(t("common-load-error")),
      );
    const paths = () => lotReads(page.reads).map((url) => url.pathname + url.search);
    const path = `/api/v1/auctions/${AUCTION_ID}/lots?page=1&page_size=20&fields=summary`;
    try {
      dom.attach(page.find);
      await waitFor(() => table() !== null);
      page.server.readDirectory = () => {
        if (paths().length === 2) return reject("rate_limited", 429);
        if (paths().length === 3) return pending.promise;
        return null;
      };
      jest.useFakeTimers();
      onTestFinished(() => jest.useRealTimers());
      dispatch(
        page.need((element) => hasClass(element, "public-connection-refresh")),
        "Click",
      );
      jest.advanceTimersByTime(80);
      await settleTimedReads();
      expect(paths()).toEqual([path, path]);
      const prefetch = prefetchCurrentLots(
        AUCTION_ID,
        null,
        null,
        catalogLevelFilterDefault(),
        1,
        20,
        controller.signal,
      );
      await settleTimedReads();
      expect(paths()).toEqual([path, path, path]);
      jest.advanceTimersByTime(1_000);
      await settleTimedReads();
      expect(paths()).toEqual([path, path, path]);
      expect(error()).toBeNull();

      if (state === "revoked") invalidateAfterCommand();
      if (state !== "visible") {
        Object.assign(document, { hidden: true });
        document.dispatchEvent(new Event("visibilitychange"));
      }
      pending.resolve(reject("internal", 500));
      await prefetch;
      await settleTimedReads();
      expect(paths()).toEqual(Array(state === "visible" ? 4 : 3).fill(path));
      expect(error() !== null).toBe(state === "revoked");
      expect(table() !== null).toBe(state !== "revoked");
    } finally {
      controller.abort();
      pending.resolve(reject("internal", 500));
      page.dispose();
      dom.restore();
    }
  },
);

test.each(["empty", "live", "preview", "preview-error"] as const)(
  "往期看板仅在当前场次允许切换时读取所属世界的结算目录（%s）",
  async (current) => {
    const page = await mountCompetition();
    try {
      await waitFor(
        () => page.find((element) => hasClass(element, "public-auction-catalog__table")) !== null,
      );
      const round = {
        id: AUCTION_ID,
        world_id: "main",
        status: "settled",
        sequence: 12,
        scheduled_open_at: OPEN_AT,
        effective_end_at: END_AT,
        lot_count: 0,
        sold_count: 0,
        unsold_count: 0,
        sold_amount: 0,
        state_version: 5,
      };
      page.server.currentEmpty = current === "empty";
      const preview = current === "preview" || current === "preview-error";
      if (preview) {
        page.server.readCurrent = () =>
          success({
            auction: {
              id: "0190f2b4-1c2d-4e5f-8a6b-000000000000",
              world: "main",
              status: "preview",
              number: 13,
              opens_at: OPEN_AT,
              scheduled_ends_at: null,
              presentation_mode: "preview",
              bidding_open: false,
              preview_revision: 0,
              proxy_default_mode: "fast",
            },
          });
      }
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
          page: Number(url.searchParams.get("page")),
          page_size: Number(url.searchParams.get("page_size")),
          category_counts: {
            all: 0,
            ...Object.fromEntries(EQUIPMENT_CATEGORY_CODES.map(([key]) => [key, 0])),
          },
        });
      page.server.readRounds = (url) =>
        url.searchParams.has("world") && current !== "preview"
          ? reject("internal_error", 500)
          : success({ auctions: [round], page: 1, page_size: 200, total: 1 });
      invalidateAll();
      page.go(`/?auction=${AUCTION_ID}`);
      await settle();
      await waitFor(
        () => page.find((element) => hasClass(element, "public-auction-page")) !== null,
      );
      await settle();
      const switchReads = page.reads.filter(
        (path) =>
          path.startsWith("/api/v1/auctions?") && path.includes("status=settled&world=main"),
      );
      const switcher = page.find((element) => hasClass(element, "public-auction-view-switch"));
      expect(switchReads).toHaveLength(preview ? 1 : 0);
      if (current === "preview-error") {
        expect(renderedText(switcher)).toContain(t("data-error"));
      } else if (current === "preview") {
        expect(renderedText(switcher)).toContain(t("auction-history-current"));
      } else {
        expect(switcher).toBeNull();
      }
    } finally {
      page.dispose();
    }
  },
);
