// 目录列设置：类型工具栏右端的图标按钮与下拉面板（标题、全选、逐列勾选）。
// 只改写本地列偏好，不改变目录查询、URL 或竞价状态；隐藏列不卸载表格命令宿主。

import { focusElementById } from "@auction/web-shared/platform/a11y";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { unreachable } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import {
  type CatalogColumn,
  type CatalogLayout,
  type CatalogVisibility,
  catalogLayoutColumns,
  catalogVisibilityContains,
  catalogVisibilityForLayout,
  catalogVisibilitySave,
  catalogVisibilityVisibleCount,
  catalogVisibilityWithAllColumns,
  catalogVisibilityWithColumn,
  catalogVisibilityWithOnlyItem,
} from "./state";

const TOGGLE_ID = "public-catalog-columns-toggle";
const PANEL_ID = "public-catalog-columns-panel";
const TITLE_ID = "public-catalog-columns-title";
const ALL_ID = "public-catalog-column-all";

/** 列设置入口与下拉面板。展开状态只属于本组件：勾选立即生效并写入本地偏好，
 * 没有待提交的输入，所以点面板外或按 Escape 直接关闭。 */
export function CatalogColumnsMenu(props: {
  readonly visibility: Cell<CatalogVisibility>;
  readonly layout: CatalogLayout;
}): JSX.Element {
  const open = createCell(false);
  const visible = createValueMemo(() =>
    catalogVisibilityForLayout(props.visibility(), props.layout),
  );
  const shown = createValueMemo(() => catalogVisibilityVisibleCount(visible(), props.layout));
  const all = createValueMemo(() => shown() === catalogLayoutColumns(props.layout).length);
  const lastColumn = createValueMemo(() => shown() === 1);

  const close = (): void => {
    open.set(false);
    focusElementById(TOGGLE_ID);
  };

  return (
    <div class="public-catalog-columns-menu">
      <button
        id={TOGGLE_ID}
        class="public-catalog-columns__toggle"
        type="button"
        aria-controls={PANEL_ID}
        aria-expanded={open() ? "true" : "false"}
        aria-label={t("catalog-columns-settings")}
        title={t("catalog-columns-settings")}
        onClick={() => open.set(!open())}
        onKeyDown={(event) => {
          if (open() && event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            close();
          }
        }}
      >
        <ColumnsIcon />
      </button>
      <Show when={open()} fallback={<div id={PANEL_ID} hidden={true} />}>
        {/* 透明遮罩接住面板外的点击。 */}
        {/* biome-ignore lint/a11y/noStaticElementInteractions: 遮罩只承担关闭点击，键盘关闭由面板根处理。 */}
        {/* biome-ignore lint/a11y/useKeyWithClickEvents: 遮罩不单独接收键盘，面板根统一处理 Escape。 */}
        <div class="public-catalog-columns__backdrop" onClick={() => close()} />
        {/* biome-ignore lint/a11y/useSemanticElements: 源码使用 div role=group 保持 DOM 结构。 */}
        <div
          id={PANEL_ID}
          class="public-catalog-columns"
          role="group"
          aria-labelledby={TITLE_ID}
          aria-description={t("catalog-columns-hint")}
          onKeyDown={(event) => {
            if (event.key === "Escape") {
              event.preventDefault();
              event.stopPropagation();
              close();
            }
          }}
        >
          <strong id={TITLE_ID} class="public-catalog-columns__title">
            {t("catalog-columns")}
          </strong>
          <label for={ALL_ID} class="public-catalog-columns__option">
            <input
              id={ALL_ID}
              type="checkbox"
              checked={all()}
              // 目录至少保留一列，所以“全选”只有全部与部分两种状态；部分可见是混合态。
              // 原生 indeterminate 只能由脚本设置，这里用 aria-checked 表达并由样式绘制。
              aria-checked={all() ? "true" : "mixed"}
              onChange={(event) => {
                const current = props.visibility();
                // 勾选显示当前布局的全部列；取消时只保留装备列。
                const next = event.currentTarget.checked
                  ? catalogVisibilityWithAllColumns(current, props.layout)
                  : catalogVisibilityWithOnlyItem(current, props.layout);
                if (next.bits !== current.bits) {
                  props.visibility.set(next);
                  catalogVisibilitySave(next);
                }
              }}
            />
            <span>{t("catalog-columns-all")}</span>
          </label>
          <For
            each={catalogLayoutColumns(props.layout)}
            keyed={(entry) => catalogColumnInputId(entry[0])}
          >
            {(entry) => {
              const column = () => entry()[0];
              return (
                <label for={catalogColumnInputId(column())} class="public-catalog-columns__option">
                  <input
                    id={catalogColumnInputId(column())}
                    type="checkbox"
                    checked={catalogVisibilityContains(visible(), column())}
                    disabled={lastColumn() && catalogVisibilityContains(visible(), column())}
                    onChange={(event) => {
                      const current = props.visibility();
                      const next = catalogVisibilityWithColumn(
                        current,
                        props.layout,
                        column(),
                        event.currentTarget.checked,
                      );
                      if (next.bits !== current.bits) {
                        props.visibility.set(next);
                        catalogVisibilitySave(next);
                      }
                    }}
                  />
                  <span>{catalogColumnLabel(column(), props.layout)}</span>
                </label>
              );
            }}
          </For>
        </div>
      </Show>
    </div>
  );
}

/** 列表图标；1.5px stroke 与目录行操作图标一致。 */
function ColumnsIcon(): JSX.Element {
  return (
    <svg
      class="public-catalog-columns__icon"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="1.5"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01" />
    </svg>
  );
}

function catalogColumnInputId(column: CatalogColumn): string {
  switch (column) {
    case "code":
      return "public-catalog-column-code";
    case "item":
      return "public-catalog-column-item";
    case "group":
      return "public-catalog-column-group";
    case "average_roll":
      return "public-catalog-column-average-roll";
    case "level":
      return "public-catalog-column-level";
    case "attributes":
      return "public-catalog-column-attributes";
    case "seller":
      return "public-catalog-column-seller";
    case "price":
      return "public-catalog-column-price";
    case "next_bid":
      return "public-catalog-column-next-bid";
    case "leader":
      return "public-catalog-column-leader";
    case "remaining":
      return "public-catalog-column-remaining";
    case "status":
      return "public-catalog-column-status";
    case "actions":
      return "public-catalog-column-actions";
    default:
      return unreachable(column);
  }
}

function catalogColumnLabel(column: CatalogColumn, layout: CatalogLayout): string {
  switch (column) {
    case "code":
      return t("catalog-code");
    case "item":
      return t("catalog-item");
    case "group":
      return t("catalog-group");
    case "average_roll":
      return t("equipment-avg-roll");
    case "level":
      return t("catalog-level");
    case "attributes":
      return t("catalog-primary-attributes");
    case "seller":
      return t("catalog-seller");
    case "price":
      return layout === "preview" ? t("catalog-start-price") : t("catalog-current");
    case "next_bid":
      return t("catalog-next-bid");
    case "leader":
      return t("catalog-leader");
    case "remaining":
      return t("catalog-remaining");
    case "status":
      return t("catalog-status");
    case "actions":
      return t("catalog-actions");
    default:
      return unreachable(column);
  }
}
