//! 手机导航、装备分组原生选择、装备类型多选与面板草稿和账户章节折叠入口；不持有查询或命令。

import { EQUIPMENT_CATEGORY_CODES } from "@auction/contracts/dto/public/catalog/lot";
import { focusElementById } from "@auction/web-shared/platform/a11y";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { Link, type RouteMatch } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, onSettled, Show } from "solid-js";

import { catalogCategoryLabel, equipmentGroupLabel } from "../catalog/presentation";
import {
  catalogCategoryCloseOutcome,
  catalogCategoryCommitValue,
  catalogCategoryEscapeAction,
  catalogCategoryKeys,
  shouldRestoreCatalogCategoryFocus,
  toggleCatalogCategory,
} from "../query/category";
import { AccountSectionLinks, NavigationLinks } from "./index";

/** 导航开关按钮的元素 ID；在导航内按 Escape 收起时按 ID 把焦点还给它。 */
const NAVIGATION_TOGGLE_ID = "public-navigation-toggle";

const CATEGORY_TOGGLE_ID = "public-catalog-category";
const CATEGORY_PANEL_ID = "public-catalog-category-panel";
const CATEGORY_LABEL_ID = "public-catalog-category-label";

export function Navigation(props: {
  auctionHomeHref: string;
  currentRoute: RouteMatch;
  scopedHref: (path: string) => string;
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
          currentRoute={props.currentRoute}
          scopedHref={props.scopedHref}
          resetHomeQuery={props.resetHomeQuery}
          onHome={props.onHome}
        />
        {/* Solid 版专属：语言与主题在手机上移到设置页，入口只出现在手机导航里。 */}
        <Link
          href={props.scopedHref("/settings")}
          aria-current={
            props.currentRoute.kind === "matched" && props.currentRoute.name === "Settings"
              ? "page"
              : "false"
          }
        >
          {t("page-settings-title")}
        </Link>
      </nav>
    </>
  );
}

/** Solid 版装备类型用可多选面板；Rust 版仍是原生单选。面板内的勾选只改草稿，关闭时才生效：
 * 应用分类会触发目录重读，而承载本组件的目录表会在读取期间卸载；立即写查询会丢失展开状态。
 * 计数来自同一已校验目录。 */
export function CategoryFilter(props: {
  active: string;
  /** 各分类的数量角标；缺省时不显示（寄售目录没有按分类的统计）。 */
  counts?: readonly [number, number, number, number, number, number, number, number];
  focusRequest: Cell<boolean>;
  onChange: (category: string) => boolean;
}): JSX.Element {
  const open = createCell(false);
  const draft = createCell(props.active);
  const selected = () => catalogCategoryKeys(draft());
  // 有改动的关闭会让目录表随重读卸载，旧开关也随之消失。重新挂载后只在焦点仍停在文档起点时
  // 归还，用户已经主动移到其他控件时放弃并清除请求，避免抢焦点。
  onSettled(() => {
    if (!props.focusRequest.peek()) {
      return;
    }
    props.focusRequest.set(false);
    if (shouldRestoreCatalogCategoryFocus(document.activeElement, document.body)) {
      focusElementById(CATEGORY_TOGGLE_ID);
    }
  });
  const openPanel = (): void => {
    draft.set(props.active);
    open.set(true);
  };
  const close = (): void => {
    const submission = catalogCategoryCommitValue(props.active, draft.peek());
    const applyOutcome = (written: boolean): void => {
      const outcome = catalogCategoryCloseOutcome(submission, written);
      open.set(outcome.panelOpen);
      props.focusRequest.set(outcome.focusRequest);
      if (outcome.focusToggleNow) {
        focusElementById(CATEGORY_TOGGLE_ID);
      }
    };
    if (submission === null) {
      applyOutcome(false);
      return;
    }
    // 先收面板并登记焦点请求，再写查询：成功时目录重读会同步卸载当前工具栏，失败时查询没有
    // 变化、工具栏不会重新挂载，必须撤销请求并恢复面板，否则请求会遗留到下一次无关挂载。
    open.set(false);
    props.focusRequest.set(true);
    applyOutcome(props.onChange(submission));
  };
  // 面板打开期间焦点可能被 Tab 到相邻控件；Escape 在文档级收起，收起或卸载时移除监听，
  // 避免残留监听器重复触发关闭。
  createEffect(
    () => open(),
    (panelOpen) => {
      if (!panelOpen) {
        return;
      }
      const handleKeyDown = (event: KeyboardEvent): void => {
        if (event.key !== "Escape") {
          return;
        }
        event.preventDefault();
        event.stopPropagation();
        const toggle = document.getElementById(CATEGORY_TOGGLE_ID);
        if (catalogCategoryEscapeAction(toggle?.getClientRects().length ?? 0) === "cancel") {
          draft.set(props.active);
          props.focusRequest.set(false);
          open.set(false);
          return;
        }
        close();
      };
      document.addEventListener("keydown", handleKeyDown);
      return () => document.removeEventListener("keydown", handleKeyDown);
    },
  );
  const categoryCount = (category: string): number => {
    const index = EQUIPMENT_CATEGORY_CODES.findIndex(([key]) => key === category);
    return index < 0 ? 0 : (props.counts?.[index + 1] ?? 0);
  };
  const countSuffix = (count: number): string => (props.counts === undefined ? "" : ` (${count})`);
  const summary = (): string => {
    const keys = open() ? selected() : catalogCategoryKeys(props.active);
    if (keys.length === 0) {
      return `${t("filter-all")}${countSuffix(props.counts?.[0] ?? 0)}`;
    }
    const first = keys[0];
    if (keys.length === 1 && first !== undefined) {
      return `${catalogCategoryLabel(first)}${countSuffix(categoryCount(first))}`;
    }
    const total = keys.reduce((sum, key) => sum + categoryCount(key), 0);
    return `${t("filter-category-selected", { count: keys.length })}${countSuffix(total)}`;
  };

  return (
    <div class="public-catalog-category-picker public-catalog-category-picker--multi public-mobile">
      <span id={CATEGORY_LABEL_ID}>{t("filter-equipment-type")}</span>
      <div class="public-catalog-category-menu">
        <button
          id={CATEGORY_TOGGLE_ID}
          class="public-catalog-category-menu__toggle"
          type="button"
          aria-haspopup="true"
          aria-controls={CATEGORY_PANEL_ID}
          aria-expanded={open() ? "true" : "false"}
          aria-labelledby={`${CATEGORY_LABEL_ID} ${CATEGORY_TOGGLE_ID}`}
          onClick={() => {
            if (open()) {
              close();
            } else {
              openPanel();
            }
          }}
        >
          {summary()}
        </button>
        <Show when={open()} fallback={<div id={CATEGORY_PANEL_ID} hidden={true} />}>
          {/* 透明遮罩接住面板外的点击。 */}
          {/* biome-ignore lint/a11y/noStaticElementInteractions: 遮罩只承担关闭点击，键盘关闭由文档级监听处理。 */}
          {/* biome-ignore lint/a11y/useKeyWithClickEvents: 遮罩不单独接收键盘，文档级监听统一处理 Escape。 */}
          <div class="public-catalog-category-menu__backdrop" onClick={() => close()} />
          {/* biome-ignore lint/a11y/useSemanticElements: 源码使用 div role=group 保持下拉面板结构。 */}
          <div
            id={CATEGORY_PANEL_ID}
            class="public-catalog-category-menu__panel"
            role="group"
            aria-labelledby={CATEGORY_LABEL_ID}
          >
            <label
              for="public-catalog-category-option-all"
              class="public-catalog-category-menu__option"
            >
              <input
                id="public-catalog-category-option-all"
                type="checkbox"
                checked={selected().length === 0}
                onClick={(event) => {
                  if (selected().length === 0) {
                    event.preventDefault();
                  }
                }}
                onChange={(event) => {
                  if (event.currentTarget.checked) {
                    draft.set("all");
                  }
                }}
              />
              <span>{t("filter-all")}</span>
              <Show when={props.counts}>
                {(counts) => <span class="public-catalog-category-menu__count">{counts()[0]}</span>}
              </Show>
            </label>
            <For each={EQUIPMENT_CATEGORY_CODES} keyed={(entry) => entry[0]}>
              {(entry, index) => {
                const category = () => entry()[0];
                const inputId = () => `public-catalog-category-option-${category()}`;
                return (
                  <label for={inputId()} class="public-catalog-category-menu__option">
                    <input
                      id={inputId()}
                      type="checkbox"
                      checked={selected().includes(category())}
                      onChange={() =>
                        draft.update((value) => toggleCatalogCategory(value, category()))
                      }
                    />
                    <span>{catalogCategoryLabel(category())}</span>
                    <Show when={props.counts}>
                      {(counts) => (
                        <span class="public-catalog-category-menu__count">
                          {counts()[index() + 1]}
                        </span>
                      )}
                    </Show>
                  </label>
                );
              }}
            </For>
            <button
              type="button"
              class="public-catalog-category-menu__done"
              onClick={() => close()}
            >
              {t("filter-done")}
            </button>
          </div>
        </Show>
      </div>
    </div>
  );
}

/** 手机装备分组使用原生选择器；与桌面分组标签消费同一当前值和回调。 */
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
