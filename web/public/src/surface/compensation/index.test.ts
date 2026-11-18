// 生产补偿池页面在批次纠偏和场次恢复时的地址与 HTTP 边界。

import { expect, test } from "bun:test";
import { t } from "@auction/web-shared/ui/i18n";

import {
  AUCTION_ID,
  dispatch,
  hasClass,
  mountCompetition,
  renderedText,
  settle,
  waitFor,
} from "../commands/competition-fixture.test";
import { success } from "../shell/session-fixture";

const ENDPOINT = "/api/v1/compensation-pool";
const ROUND = {
  auction_id: AUCTION_ID,
  world_id: "main",
  auction_sequence: 7,
  policy_mode: "hold_curve_v1",
  opening_balance: 0,
  tax_income: 0,
  discount_demand: 0,
  compensation_out: 0,
  closing_balance: 0,
  movement_count: 0,
  reversal_count: 0,
  reversal_amount: 0,
  settled_at: "2026-11-16T00:00:00Z",
};

test.each([
  ["auction=bad", "clear"],
  [`auction=${AUCTION_ID}&a%75ction=${AUCTION_ID}`, "select"],
] as const)("补偿池先独立校正批次页码，非法场次 %s 等显式恢复", async (raw, action) => {
  const page = await mountCompetition("main", true);
  const fields = `?${raw}&movement_page=3&keep=%2b%20x`;
  const batches = () => page.reads.filter((read) => read.startsWith(`${ENDPOINT}?`));
  const details = () => page.reads.filter((read) => read.startsWith(`${ENDPOINT}/`));
  const auctionAtDetailRead: string[][] = [];
  const alert = () =>
    page.find(
      (element) =>
        element.getAttribute("role") === "alert" &&
        renderedText(element) === t("auction-invalid-link"),
    );
  try {
    page.server.readPage = (url) => {
      const requestedPage = Number(url.searchParams.get("page"));
      const pageSize = Number(url.searchParams.get("page_size"));
      if (url.pathname === ENDPOINT) {
        return success({
          balance: 0,
          updated_at: ROUND.settled_at,
          rounds: requestedPage === 1 ? [ROUND] : [],
          page: requestedPage,
          page_size: pageSize,
          total: 1,
        });
      }
      if (url.pathname !== `${ENDPOINT}/${AUCTION_ID}/lots`) return null;
      auctionAtDetailRead.push(page.location.searchParams.getAll("auction"));
      return success({
        round: ROUND,
        lots: [],
        page: requestedPage,
        page_size: pageSize,
        total: 0,
      });
    };
    page.go(`/compensation-pool${fields}&page=2`);
    await waitFor(() => batches().length === 2);
    await settle();
    expect(batches()).toEqual([
      `${ENDPOINT}?page=2&page_size=20`,
      `${ENDPOINT}?page=1&page_size=20`,
    ]);
    expect(page.location.search).toBe(fields);
    expect(details()).toEqual([]);
    expect(alert()).not.toBeNull();
    const recover = page.need(
      (element) =>
        element.tag === "button" && renderedText(element) === t("auction-clear-selection"),
    );
    const expand = () => page.need((element) => hasClass(element, "public-compensation-expand"));
    const apply = () => dispatch(action === "clear" ? recover : expand(), "Click");
    page.rejectWrites("push");
    apply();
    await settle();
    expect(page.location.search).toBe(fields);
    expect(alert()).not.toBeNull();
    expect(details()).toEqual([]);

    page.rejectWrites(false);
    apply();
    await settle();
    expect(alert()).toBeNull();
    expect(page.location.search).toContain("keep=%2b%20x");
    if (action === "clear") {
      expect(page.location.searchParams.has("auction")).toBe(false);
      expect(details()).toEqual([]);
      dispatch(expand(), "Click");
    }
    await waitFor(() => details().length !== 0);
    expect(page.location.searchParams.getAll("auction")).toEqual([AUCTION_ID]);
    expect(page.location.searchParams.has("movement_page")).toBe(false);
    expect(details()).toEqual([`${ENDPOINT}/${AUCTION_ID}/lots?page=1&page_size=20`]);
    expect(auctionAtDetailRead).toEqual([[AUCTION_ID]]);
    expect(batches()).toHaveLength(2);
  } finally {
    page.dispose();
  }
});
