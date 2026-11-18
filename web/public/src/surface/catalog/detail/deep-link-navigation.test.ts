// 从实际提示链接定位目录或只读抽屉，并在 HTTP 边界区分拍品不存在与暂时失败。

import { expect, test } from "bun:test";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import { invalidateAll } from "../../../api/swr-cache";
import {
  AUCTION_ID,
  bidReceipt,
  type CompetitionPage,
  confirm,
  dispatch,
  hasClass,
  LOT_ID,
  lotDetailReceipt,
  mountCompetition,
  openBid,
  renderedText,
  SECOND_LOT_ID,
  settle,
  waitFor,
} from "../../commands/competition-fixture.test";
import { type DomElement, deferLotPage } from "../../shell/navigation-fixture.test";
import { reject } from "../../shell/session-fixture";

async function record(page: CompetitionPage, code = "Clo01"): Promise<void> {
  await openBid(page, code);
  const first = await confirm(
    page,
    page.need((element) => element.tag === "form" && hasClass(element, "public-auction-bid-form")),
  );
  first.respond(reject("temporarily_unavailable", 503));
  await waitFor(
    () =>
      page.find(
        (element) =>
          hasClass(element, "public-command-notices__item") &&
          page.find(
            (link) =>
              link.tag === "a" &&
              link.getAttribute("aria-label") === t("catalog-view-lot", { code }),
            element,
          ) !== null,
      ) !== null,
  );
  dispatch(
    page.need((element) => hasClass(element, "ui-dialog__close")),
    "Click",
  );
}

function viewCommandLot(page: CompetitionPage, lotId = LOT_ID): void {
  const section = page.need((element) => hasClass(element, "public-command-notices"));
  const link = page.need(
    (element) => element.tag === "a" && element.attributes.href === `/lots/${lotId}`,
    section,
  );
  dispatch(link, "Click");
}

async function openedDrawer(page: CompetitionPage, title: string) {
  const matches = (element: DomElement) =>
    hasClass(element, "public-catalog-detail__title") && element.attributes.title === title;
  await waitFor(() => page.find(matches) !== null);
  return page.need((element) => hasClass(element, "public-catalog-detail"));
}

test.each(["row", "round-unavailable", "directory-unavailable"] as const)(
  "提示区链接按冻结世界和场次打开目标抽屉（%s）",
  async (destination) => {
    const page = await mountCompetition("isekai");
    try {
      await record(page);
      page.go("/login");
      if (destination === "round-unavailable") {
        page.server.currentEmpty = true;
        invalidateAll();
      } else if (destination === "directory-unavailable") {
        page.server.directoryFailed = true;
      }
      viewCommandLot(page);
      const drawer = await openedDrawer(page, "Legendary Cotton Robe");
      expect(page.location.pathname).toBe("/");
      expect(page.location.searchParams.get("world")).toBe("isekai");
      expect(page.location.searchParams.get("auction")).toBe(AUCTION_ID);
      const recovery = page.find(
        (element) => hasClass(element, "public-catalog-detail__bid"),
        drawer,
      );
      if (destination === "row") {
        expect(recovery).not.toBeNull();
      } else {
        expect(recovery).toBeNull();
        await waitFor(
          () => page.find((element) => hasClass(element, "ui-async-state--error")) !== null,
        );
        expect(page.find((element) => hasClass(element, "ui-async-state--error"))).not.toBeNull();
      }
    } finally {
      page.dispose();
    }
  },
);

test("普通深链从空看板交给只读抽屉", async () => {
  const page = await mountCompetition("main", true);
  try {
    page.go(`/lots/${LOT_ID}`);
    const drawer = await openedDrawer(page, "Legendary Cotton Robe");
    expect(page.location.pathname).toBe("/");
    expect(page.location.searchParams.has("auction")).toBe(false);
    await waitFor(
      () => page.find((element) => hasClass(element, "ui-async-state--empty")) !== null,
    );
    expect(page.find((element) => hasClass(element, "ui-async-state--empty"))).not.toBeNull();
    expect(
      page.find((element) => hasClass(element, "public-catalog-detail__bid"), drawer),
    ).toBeNull();
  } finally {
    page.dispose();
  }
});

test.each([404, 410])("真实 HTTP %s 在普通直达和提示入口都显示拍品不存在", async (status) => {
  const page = await mountCompetition();
  try {
    await record(page);
    page.server.readLot = () => reject("not_found", status);
    page.go(`/lots/${LOT_ID}`);
    const missing = () =>
      page.find((element) => renderedText(element) === t("catalog-detail-not-found"));
    await waitFor(() => missing() !== null);
    expect(page.find((element) => hasClass(element, "ui-async-state--error"))).toBeNull();
    const reads = page.reads.filter((path) => path === `/api/v1/lots/${LOT_ID}`).length;
    viewCommandLot(page);
    await waitFor(
      () => page.reads.filter((path) => path === `/api/v1/lots/${LOT_ID}`).length > reads,
    );
    await waitFor(() => missing() !== null);
    expect(page.find((element) => hasClass(element, "public-catalog-detail"))).toBeNull();
    expect(page.find((element) => hasClass(element, "ui-async-state--error"))).toBeNull();
  } finally {
    page.dispose();
  }
});

test("A 的失败页切换到 B，B 交付后普通直达重新定位", async () => {
  const page = await mountCompetition("isekai");
  try {
    await record(page);
    await record(page, "Clo02");
    const originalRead = page.server.readLot;
    page.server.readLot = (id) =>
      id === LOT_ID ? reject("temporarily_unavailable", 503) : originalRead(id);
    page.go("/login");
    viewCommandLot(page);
    await waitFor(
      () => page.find((element) => hasClass(element, "ui-async-state--error")) !== null,
    );
    expect(page.location.pathname).toBe(`/lots/${LOT_ID}`);
    viewCommandLot(page, SECOND_LOT_ID);
    await openedDrawer(page, "Magnificent Cotton Robe");
    expect(page.location.searchParams.get("auction")).toBe(AUCTION_ID);

    page.go(`/lots/${SECOND_LOT_ID}`);
    await openedDrawer(page, "Magnificent Cotton Robe");
    expect(page.location.searchParams.get("world")).toBe("isekai");
    expect(page.location.searchParams.has("auction")).toBe(false);
  } finally {
    page.dispose();
  }
});

test.each([200, 404, 410, 503] as const)(
  "重放成功且目录失败后补读拍品详情（HTTP %i）",
  async (status) => {
    const page = await mountCompetition("isekai");
    const directory = Promise.withResolvers<Response>();
    try {
      await record(page);
      page.go("/login");
      page.server.readDirectory = (url) =>
        url.searchParams.has("category") ? directory.promise : null;
      viewCommandLot(page);
      await waitFor(() =>
        page.reads.some((url) => url.includes("/lots?") && url.includes("category=clo")),
      );
      expect(page.reads.filter((url) => url === `/api/v1/lots/${LOT_ID}`)).toHaveLength(1);
      expect(page.find((element) => hasClass(element, "public-catalog-detail"))).toBeNull();

      const notice = page.need((element) => hasClass(element, "public-command-notices"));
      dispatch(
        page.need(
          (element) => element.tag === "button" && renderedText(element) === t("data-retry"),
          notice,
        ),
        "Click",
      );
      await waitFor(() => page.commands.length === 2);
      page.server.readLot = (id) =>
        status === 200
          ? lotDetailReceipt(id, 4000, "Current bidder")
          : status === 503
            ? reject("temporarily_unavailable", 503)
            : reject("not_found", status);
      page.commands[1]?.respond(bidReceipt(LOT_ID, 3500));
      await waitFor(
        () => page.find((element) => hasClass(element, "public-command-notices__receipt")) !== null,
      );
      directory.resolve(reject("not_found", 404));

      if (status !== 200) {
        const missing = status === 404 || status === 410;
        const expectedText = t(missing ? "catalog-detail-not-found" : "data-error");
        const failure = (element: DomElement) =>
          hasClass(element, missing ? "ui-async-state--empty" : "ui-async-state--error") &&
          renderedText(element).includes(t("catalog-detail")) &&
          renderedText(element).includes(expectedText);
        await waitFor(() => page.find(failure) !== null);
        const panel = page.need(failure);
        expect(panel.getAttribute("role")).toBe(missing ? "status" : "alert");
        expect(page.reads.filter((url) => url === `/api/v1/lots/${LOT_ID}`)).toHaveLength(2);
        expect(page.find((element) => hasClass(element, "public-catalog-detail"))).toBeNull();
        const retry = (element: DomElement) => hasClass(element, "ui-async-state__retry");
        if (missing) {
          expect(page.find(retry, panel)).toBeNull();
          return;
        }
        const directoryReads = page.reads.filter((url) => url.includes("/lots?")).length;
        const detailReads = page.reads.filter((url) => url === `/api/v1/lots/${LOT_ID}`).length;
        page.server.readLot = (id) => lotDetailReceipt(id, 4000, "Current bidder");
        dispatch(page.need(retry, panel), "Click");
        expect(page.find(failure)).toBeNull();
        await waitFor(
          () => page.reads.filter((url) => url === `/api/v1/lots/${LOT_ID}`).length > detailReads,
        );
        expect(page.reads.filter((url) => url.includes("/lots?"))).toHaveLength(directoryReads);
        expect(page.commands).toHaveLength(2);
      }
      const drawer = await openedDrawer(page, "Legendary Cotton Robe");
      await settle();
      expect(
        page.find((element) => hasClass(element, "public-catalog-detail__bid"), drawer),
      ).toBeNull();
      expect(renderedText(drawer)).toContain(formatCreditAmount(4000));
      expect(renderedText(drawer)).toContain("Current bidder");
      expect(renderedText(drawer)).not.toContain(formatCreditAmount(1500));
      expect(page.reads.filter((url) => url === `/api/v1/lots/${LOT_ID}`).length).toBeGreaterThan(
        1,
      );
    } finally {
      directory.resolve(reject("not_found", 404));
      page.dispose();
    }
  },
);

test.each([false, true])("目录加载期间的打开请求只交给目标历史条目（后退 %s）", async (back) => {
  const page = await mountCompetition("isekai");
  const directory = Promise.withResolvers<Response>();
  const drawer = (element: DomElement) => hasClass(element, "public-catalog-detail");
  const boardTable = (element: DomElement) => hasClass(element, "public-auction-catalog__table");
  try {
    await record(page);
    const originalUrl = page.location.href;
    const readCount = page.reads.length;
    page.server.readDirectory = (url) =>
      url.searchParams.has("category") ? directory.promise : null;
    viewCommandLot(page);
    await waitFor(
      () =>
        page.location.searchParams.get("auction") === AUCTION_ID &&
        page.reads
          .slice(readCount)
          .some((url) => url.includes("/lots?") && url.includes("category=clo")),
    );
    expect(page.find(drawer)).toBeNull();
    if (back) {
      page.back();
      expect(page.location.href).toBe(originalUrl);
    }
    directory.resolve(reject("not_found", 404));
    if (!back) {
      await openedDrawer(page, "Legendary Cotton Robe");
      return;
    }

    await waitFor(() => page.find(boardTable) !== null);
    await settle();
    expect(page.find(drawer)).toBeNull();
    page.forward();
    expect(page.location.searchParams.get("auction")).toBe(AUCTION_ID);
    await waitFor(
      () => page.find((element) => hasClass(element, "ui-async-state--error")) !== null,
    );
    expect(page.find(drawer)).toBeNull();
  } finally {
    directory.resolve(reject("not_found", 404));
    page.dispose();
  }
});

test.each([true, false])("冷加载期间替换导航后，普通深链重新定位（中途离页 %s）", async (leave) => {
  const page = await mountCompetition("isekai");
  const chunk = await deferLotPage();
  try {
    await record(page);
    page.go("/login");
    viewCommandLot(page);
    await chunk.started;
    expect(page.location.pathname).toBe(`/lots/${LOT_ID}`);
    expect(page.reads).not.toContain(`/api/v1/lots/${LOT_ID}`);
    if (leave) page.go("/login");
    page.go(`/lots/${LOT_ID}`);
    chunk.release();
    await openedDrawer(page, "Legendary Cotton Robe");
    expect(page.location.searchParams.get("world")).toBe("isekai");
    expect(page.location.searchParams.has("auction")).toBe(false);
  } finally {
    chunk.restore();
    page.dispose();
  }
});
