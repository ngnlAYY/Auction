//! 桌面装备类型标签；只消费页面投影与事件。桌面侧栏导航见 `presentation::console`，
//! 账户章节入口只在 `presentation::mobile`。

import { EQUIPMENT_CATEGORY_CODES } from "@auction/contracts/dto/public/catalog/lot";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For } from "solid-js";

import { catalogCategoryLabel } from "../catalog/presentation";

export function CategoryFilter(props: {
  active: string;
  counts: readonly [number, number, number, number, number, number, number, number];
  onChange: (category: string) => void;
}): JSX.Element {
  return (
    <nav class="public-auction-filter public-desktop" aria-label={t("filter-label")}>
      <span class="public-auction-filter__label">{t("filter-equipment-type")}</span>
      <button
        type="button"
        class="public-auction-filter__tab"
        aria-current={props.active === "all" ? "true" : "false"}
        onClick={() => props.onChange("all")}
      >
        {t("filter-all")}
        <span class="public-auction-filter__count">{props.counts[0]}</span>
      </button>
      <For each={EQUIPMENT_CATEGORY_CODES} keyed={false}>
        {(entry, index) => {
          const category = entry()[0];
          return (
            <button
              type="button"
              class="public-auction-filter__tab"
              aria-current={props.active === category ? "true" : "false"}
              onClick={() => props.onChange(category)}
            >
              {catalogCategoryLabel(category)}
              <span class="public-auction-filter__count">{props.counts[index + 1]}</span>
            </button>
          );
        }}
      </For>
    </nav>
  );
}
