// 价格历史筛选器把完整装备名转换为名称查询。

import { expect, test } from "bun:test";

const { historyEquipmentNameQuery } = await import("./validation");

test("equipment_name_within_the_term_limit_is_queried_as_is：词数上限内的装备名原样查询", () => {
  expect(historyEquipmentNameQuery("  Legendary Charged Cotton Gloves of the Fire-eater ")).toBe(
    "Legendary Charged Cotton Gloves of the Fire-eater",
  );
});

test("equipment_name_over_the_term_limit_keeps_the_longest_terms_in_order：超过词数上限时按原顺序保留最长的词", () => {
  expect(
    historyEquipmentNameQuery(
      "Legendary Ethereal Mithril Power Helmet of the Heaven-sent Slaughter",
    ),
  ).toBe("Legendary Ethereal Mithril Power Helmet the Heaven-sent Slaughter");
  // 等长的词保留靠前者：九个等长词丢弃最后一个。
  expect(historyEquipmentNameQuery("aa bb cc dd ee ff gg hh ii")).toBe("aa bb cc dd ee ff gg hh");
});

test("equipment_name_without_usable_terms_yields_no_query：没有可用词条时不出查询", () => {
  expect(historyEquipmentNameQuery("")).toBeNull();
  expect(historyEquipmentNameQuery(" , \t")).toBeNull();
  expect(historyEquipmentNameQuery("Legendary\u0007Axe")).toBeNull();
  expect(historyEquipmentNameQuery("x".repeat(129))).toBeNull();
});
