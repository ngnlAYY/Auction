//! 桌面装备类型标签；只消费页面投影与事件。桌面侧栏导航见 `presentation::console`，
//! 账户章节入口只在 `presentation::mobile`。

import { EQUIPMENT_CATEGORY_CODES } from "@auction/contracts/dto/public/catalog/lot";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For } from "solid-js";

import { catalogCategoryLabel } from "../catalog/presentation";
import { catalogCategoryKeys, toggleCatalogCategory } from "../query";

/** Solid 版分类标签是多选开关；Rust 版仍为单选。回调收到新的当前值。 */
export function CategoryFilter(props: {
  active: string;
  counts: readonly [number, number, number, number, number, number, number, number];
  onChange: (category: string) => boolean;
}): JSX.Element {
  return (
    <nav class="public-auction-filter public-desktop" aria-label={t("filter-label")}>
      <span class="public-auction-filter__label">{t("filter-equipment-type")}</span>
      <button
        type="button"
        class="public-auction-filter__tab"
        aria-pressed={catalogCategoryKeys(props.active).length === 0 ? "true" : "false"}
        aria-current={catalogCategoryKeys(props.active).length === 0 ? "true" : "false"}
        onClick={() => props.onChange("all")}
      >
        {t("filter-all")}
        <span class="public-auction-filter__count">{props.counts[0]}</span>
      </button>
      <For each={EQUIPMENT_CATEGORY_CODES} keyed={(entry) => entry[0]}>
        {(entry, index) => {
          const category = () => entry()[0];
          const selected = () => catalogCategoryKeys(props.active).includes(category());
          return (
            <button
              type="button"
              class="public-auction-filter__tab"
              aria-pressed={selected() ? "true" : "false"}
              aria-current={selected() ? "true" : "false"}
              onClick={() => props.onChange(toggleCatalogCategory(props.active, category()))}
            >
              {catalogCategoryLabel(category())}
              <span class="public-auction-filter__count">{props.counts[index() + 1]}</span>
            </button>
          );
        }}
      </For>
    </nav>
  );
}
