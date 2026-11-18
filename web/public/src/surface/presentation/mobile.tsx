//! 手机导航、原生装备分组/类型选择和账户章节折叠入口；不持有查询、草稿或命令。

import { EQUIPMENT_CATEGORY_CODES } from "@auction/contracts/dto/public/catalog/lot";
import { focusElementById } from "@auction/web-shared/platform/a11y";
import { Link } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For } from "solid-js";

import { catalogCategoryLabel, equipmentGroupLabel } from "../catalog/presentation";
import { AccountSectionLinks, NavigationLinks } from "./index";

/** 导航开关按钮的元素 ID；在导航内按 Escape 收起时按 ID 把焦点还给它。 */
const NAVIGATION_TOGGLE_ID = "public-navigation-toggle";

export function Navigation(props: {
  auctionHomeHref: string;
  resetHomeQuery: boolean;
  open: boolean;
  onToggle: () => void;
  onClose: () => void;
  onHome: () => void;
}): JSX.Element {
  return (
    <>
      <button
        id={NAVIGATION_TOGGLE_ID}
        class="public-shell__nav-toggle public-mobile"
        type="button"
        aria-controls="public-primary-navigation"
        aria-expanded={props.open ? "true" : "false"}
        onClick={() => props.onToggle()}
        onKeyDown={(event) => {
          if (props.open && event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            props.onClose();
          }
        }}
      >
        {t("nav-label")}
      </button>
      <nav
        id="public-primary-navigation"
        class={
          props.open
            ? "public-shell__nav public-shell__nav--open public-mobile"
            : "public-shell__nav public-mobile"
        }
        aria-label={t("nav-label")}
        onClick={() => props.onClose()}
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            props.onClose();
            focusElementById(NAVIGATION_TOGGLE_ID);
          }
        }}
      >
        <NavigationLinks
          auctionHomeHref={props.auctionHomeHref}
          resetHomeQuery={props.resetHomeQuery}
          onHome={props.onHome}
        />
        {/* Solid 版专属：语言与主题在手机上移到设置页，入口只出现在手机导航里。 */}
        <Link href="/settings">{t("page-settings-title")}</Link>
      </nav>
    </>
  );
}

/** 八个类型选项用原生选择器替代多行标签；计数仍来自同一已校验目录。 */
export function CategoryFilter(props: {
  active: string;
  counts: readonly [number, number, number, number, number, number, number, number];
  onChange: (category: string) => void;
}): JSX.Element {
  return (
    <label class="public-catalog-category-picker public-mobile" for="public-catalog-category">
      <span>{t("filter-equipment-type")}</span>
      <select
        id="public-catalog-category"
        name="category"
        value={props.active}
        onChange={(event) => props.onChange(event.currentTarget.value)}
      >
        <option value="all">
          {t("filter-all")} ({props.counts[0]})
        </option>
        <For each={EQUIPMENT_CATEGORY_CODES} keyed={false}>
          {(entry, index) => {
            const category = entry()[0];
            return (
              <option value={category}>
                {catalogCategoryLabel(category)} ({props.counts[index + 1]})
              </option>
            );
          }}
        </For>
      </select>
    </label>
  );
}

/** 手机装备分组与类型同为原生选择器；与桌面分组标签消费同一当前值和回调。 */
export function EquipmentGroupFilter(props: {
  active: string;
  onChange: (group: string) => void;
}): JSX.Element {
  return (
    <label
      class="public-catalog-category-picker public-mobile"
      for="public-catalog-equipment-group"
    >
      <span>{t("equipment-group")}</span>
      <select
        id="public-catalog-equipment-group"
        name="equipment_group"
        value={props.active}
        onChange={(event) => props.onChange(event.currentTarget.value)}
      >
        <option value="all">{t("filter-all")}</option>
        <For each={["rare", "fusion"] as const} keyed={false}>
          {(group) => <option value={group()}>{equipmentGroupLabel(group())}</option>}
        </For>
      </select>
    </label>
  );
}

export function AccountNavigation(): JSX.Element {
  return (
    <details class="public-account-navigation public-mobile">
      <summary>{t("account-section-navigation")}</summary>
      <nav aria-label={t("account-section-navigation")}>
        <AccountSectionLinks />
      </nav>
    </details>
  );
}

/** 长筛选面板底部仍提交原表单，不创建第二套草稿或查询。 */
export function HistoryApply(): JSX.Element {
  return (
    <button type="submit" class="public-history-filter-actions__apply public-mobile">
      {t("history-filter-apply")}
    </button>
  );
}
