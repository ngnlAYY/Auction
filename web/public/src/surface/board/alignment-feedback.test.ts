// 看板在生产目录的自动对齐重读用尽后，同时显示旧预览和重试。

import { expect, test } from "bun:test";
import { t } from "@auction/web-shared/ui/i18n";
import {
  AUCTION_ID,
  dispatch,
  hasClass,
  LOT_ID,
  mountCompetition,
  renderedText,
  waitFor,
} from "../commands/competition-fixture.test";
import { success } from "../shell/session-fixture";

const OPEN_AT = "2036-01-01T00:00:00Z";

test.each([true, false])("旧预览保留，只有对齐重读用尽才提示（自动对齐 %s）", async (aligned) => {
  const tableName = (title: string) =>
    page.find(
      (element) =>
        hasClass(element, "public-auction-catalog__table") && renderedText(element).includes(title),
    );
  const oldTitle = "Legendary Cotton Robe";
  const newTitle = "Peerless Cotton Robe";
  const reread = Promise.withResolvers<Response>();
  let revision = 1;
  let readCount = 0;
  const previewDirectory = (listRevision: number) =>
    success({
      auction_id: AUCTION_ID,
      auction_status: "preview",
      projection_kind: "preview",
      presentation_mode: "preview",
      bidding_open: false,
      opens_at: OPEN_AT,
      preview_revision: listRevision,
      lots: [
        {
          id: LOT_ID,
          auction_id: AUCTION_ID,
          code: "Clo01",
          status: "pending",
          seller_name: "Seller",
          start_price: 1000,
          equipment_name: listRevision === 1 ? oldTitle : newTitle,
          equipment_group: "rare",
          equipment_category: "Cloth",
          equipment_level: 450,
          primary_attribute_initials: "IW",
          avg_roll: 70,
          game_equipment_url: "https://hentaiverse.org/equip/123456/0123456789",
        },
      ],
      total: 1,
      page: 1,
      page_size: 20,
      category_counts: { all: 1, one: 0, two: 0, sta: 0, shd: 0, clo: 1, lig: 0, hea: 0 },
    });
  const page = await mountCompetition("main", false, {
    href: "/",
    readCurrent: () =>
      success({
        auction: {
          id: AUCTION_ID,
          world: "main",
          number: 12,
          status: "preview",
          presentation_mode: "preview",
          opens_at: OPEN_AT,
          scheduled_ends_at: null,
          bidding_open: false,
          preview_revision: revision,
          proxy_default_mode: "fast",
        },
      }),
    readDirectory: () => {
      readCount += 1;
      return readCount === 3 ? reread.promise : previewDirectory(readCount >= 4 ? 2 : 1);
    },
  });
  const feedback = () =>
    page.find(
      (element) =>
        element.getAttribute("role") === "alert" &&
        renderedText(element).includes(t("catalog-preview-stale")),
    );
  try {
    await waitFor(() => tableName(oldTitle) !== null);
    expect(feedback()).toBeNull();
    const address = page.location.href;
    revision = 2;
    dispatch(
      page.need((element) => hasClass(element, "public-connection-refresh")),
      "Click",
    );
    await waitFor(() => readCount === 3);
    expect(tableName(oldTitle)).not.toBeNull();
    expect(feedback()).toBeNull();

    reread.resolve(previewDirectory(aligned ? 2 : 1));
    if (!aligned) {
      await waitFor(() => feedback() !== null);
      const banner = feedback();
      if (banner === null) throw new Error("未挂载预览过期提示");
      expect(tableName(oldTitle)).not.toBeNull();
      dispatch(
        page.need((element) => hasClass(element, "ui-async-state__retry"), banner),
        "Click",
      );
    }
    await waitFor(() => tableName(newTitle) !== null);
    expect(feedback()).toBeNull();
    expect(tableName(oldTitle)).toBeNull();
    expect(
      page.reads.filter((path) => path.startsWith(`/api/v1/auctions/${AUCTION_ID}/lots?`)),
    ).toHaveLength(aligned ? 3 : 4);
    expect(page.location.href).toBe(address);
  } finally {
    reread.resolve(previewDirectory(1));
    page.dispose();
  }
});
