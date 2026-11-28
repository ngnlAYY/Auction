// 目录分组与等级筛选；草稿局部持有，加载或错误不卸载输入，查询由看板写回 URL。

import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, Show } from "solid-js";
import type { CatalogLevelFilter } from "../../api/model";
import { catalogLevelFilterParse } from "../../api/model";
import { WorldTabs } from "../board/world";
import { EquipmentGroupFilter } from "../presentation/mobile";
import { currentPublicPathname } from "../query";
import { equipmentGroupLabel } from "./presentation";

/** 分组、包含式等级上限和无等级开关共用一行；只表达读取条件，不修改装备或竞价资格。
 * `children` 渲染在工具栏右端，供拍卖页放置预览与往期场次的视图切换。 */
export function CatalogGroupFilters(props: {
  readonly activeEquipmentGroup: string;
  readonly maxLevel: string | null;
  readonly includeUnleveled: string;
  readonly onEquipmentGroupChange: (group: string) => void;
  readonly onLevelChange: (filter: CatalogLevelFilter) => void;
  readonly children: JSX.Element;
}): JSX.Element {
  const maximum = createCell(props.maxLevel ?? "");
  const include = createCell(props.includeUnleveled === "true");
  const invalid = createCell(
    !catalogLevelFilterParse(props.maxLevel ?? "", props.includeUnleveled).ok,
  );
  // 复选框切换在同一任务内先触发输入框失焦：失焦提交延后到微任务，由 change 取消。
  let blurApplyToken = 0;
  // 只在 URL 条件改变时恢复草稿；同条件后台刷新不能打断正在输入的等级。
  const levelDependency = createValueMemo(() => [props.maxLevel, props.includeUnleveled] as const);
  createEffect(levelDependency, ([maximumValue, includeValue]) => {
    const nextMaximum = maximumValue ?? "";
    invalid.set(!catalogLevelFilterParse(nextMaximum, includeValue).ok);
    maximum.set(nextMaximum);
    include.set(includeValue === "true");
  });
  const apply = (includeUnleveled: boolean): void => {
    const parsed = catalogLevelFilterParse(maximum(), includeUnleveled ? "true" : "false");
    if (parsed.ok) {
      invalid.set(false);
      props.onLevelChange(parsed.value);
    } else {
      invalid.set(true);
    }
  };
  return (
    <div class="public-auction-group-toolbar">
      {/* 世界切换与装备分组同一行（Solid 版专属）；只有一个世界可选时整组不渲染。 */}
      <WorldTabs />
      {/* 桌面分组标签与手机原生选择器由 CSS 二选一，共用当前分组与回调。 */}
      <nav
        class="public-auction-filter public-desktop"
        aria-label={t("filter-equipment-group-label")}
      >
        <span class="public-auction-filter__label">{t("equipment-group")}</span>
        <For each={["all", "rare", "fusion"] as const} keyed={false}>
          {(equipmentGroup) => (
            <button
              class="public-auction-filter__tab"
              type="button"
              aria-current={props.activeEquipmentGroup === equipmentGroup() ? "true" : "false"}
              onClick={() => props.onEquipmentGroupChange(equipmentGroup())}
            >
              {equipmentGroup() === "all" ? t("filter-all") : equipmentGroupLabel(equipmentGroup())}
            </button>
          )}
        </For>
      </nav>
      <EquipmentGroupFilter
        active={props.activeEquipmentGroup}
        onChange={props.onEquipmentGroupChange}
      />
      <form
        class="public-catalog-level-filter"
        method="get"
        action={currentPublicPathname()}
        onSubmit={(event) => {
          event.preventDefault();
          apply(include());
        }}
      >
        <div class="public-catalog-level-filter__controls">
          <label class="public-catalog-level-filter__maximum" for="public-catalog-max-level">
            <span>{t("filter-max-level")}</span>
            <input
              id="public-catalog-max-level"
              name="max_level"
              type="text"
              inputmode="numeric"
              enterkeyhint="done"
              autocomplete="off"
              spellcheck="false"
              maxlength="19"
              value={maximum()}
              placeholder={t("filter-level-unlimited")}
              aria-invalid={invalid() ? "true" : "false"}
              aria-description={t("filter-level-hint")}
              aria-describedby={invalid() ? "public-catalog-level-error" : ""}
              onInput={(event) => {
                maximum.set(event.currentTarget.value);
                invalid.set(false);
              }}
              onBlur={() => {
                const token = blurApplyToken + 1;
                blurApplyToken = token;
                queueMicrotask(() => {
                  if (token === blurApplyToken) apply(include());
                });
              }}
            />
          </label>
          <label
            class="public-catalog-level-filter__unleveled"
            for="public-catalog-include-unleveled"
          >
            <input
              id="public-catalog-include-unleveled"
              name="include_unleveled"
              type="checkbox"
              checked={include()}
              aria-description={t("filter-level-hint")}
              onChange={(event) => {
                blurApplyToken += 1;
                const checked = event.currentTarget.checked;
                const parsed = catalogLevelFilterParse(maximum(), checked ? "true" : "false");
                if (!parsed.ok) {
                  // 草稿等级无效时不写地址栏；复选框恢复为地址栏已生效的值。
                  invalid.set(true);
                  event.currentTarget.checked = props.includeUnleveled === "true";
                  return;
                }
                include.set(checked);
                apply(checked);
              }}
            />
            <span>{t("filter-include-unleveled")}</span>
          </label>
        </div>
        <Show when={invalid()}>
          <p
            id="public-catalog-level-error"
            class="public-catalog-level-filter__error"
            role="alert"
          >
            {t("filter-level-invalid")}
          </p>
        </Show>
      </form>
      {props.children}
    </div>
  );
}
