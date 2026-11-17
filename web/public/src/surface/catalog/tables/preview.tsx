// Public 拍卖目录的预览表格。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For } from "solid-js";

import type { CatalogRow } from "../../../api/model";
import type { CatalogVisibility } from "../columns/state";
import { catalogVisibilityContains, catalogVisibilityWidths } from "../columns/state";
import { PreviewCatalogRow } from "../rows";

/** 预览期目录表格；只展示拍品事实与查看入口，列宽度按预览布局归一化。 */
export function PreviewCatalogTable(props: {
  readonly rows: readonly CatalogRow[];
  readonly indices: readonly number[];
  readonly visibility: CatalogVisibility;
}): JSX.Element {
  const widths = createValueMemo(() => catalogVisibilityWidths(props.visibility, "preview"));
  const entries = createValueMemo(() =>
    props.indices.map((index) => {
      const row = props.rows[index];
      if (row === undefined) {
        throw new Error("目录行索引越界");
      }
      return { key: row.lotId, row };
    }),
  );
  return (
    <div class="public-auction-catalog">
      <table
        class="public-auction-catalog__table public-auction-catalog__table--preview"
        aria-label={t("auction-preview-lots")}
      >
        {/* 预览目录的操作列只有查看入口；隐藏列后按相同权重重新归一化。 */}
        <TableColumns layout="public-catalog-preview" widths={widths()} />
        <thead>
          <tr>
            <th hidden={!catalogVisibilityContains(props.visibility, "code")} scope="col">
              {t("catalog-code")}
            </th>
            <th hidden={!catalogVisibilityContains(props.visibility, "item")} scope="col">
              {t("catalog-item")}
            </th>
            <th hidden={!catalogVisibilityContains(props.visibility, "group")} scope="col">
              {t("catalog-group")}
            </th>
            <th
              hidden={!catalogVisibilityContains(props.visibility, "average_roll")}
              scope="col"
              class="public-auction-catalog__number"
            >
              {t("equipment-avg-roll")}
            </th>
            <th hidden={!catalogVisibilityContains(props.visibility, "level")} scope="col">
              {t("catalog-level")}
            </th>
            <th hidden={!catalogVisibilityContains(props.visibility, "attributes")} scope="col">
              {t("catalog-primary-attributes")}
            </th>
            <th hidden={!catalogVisibilityContains(props.visibility, "seller")} scope="col">
              {t("catalog-seller")}
            </th>
            <th
              hidden={!catalogVisibilityContains(props.visibility, "price")}
              scope="col"
              class="public-auction-catalog__number"
            >
              {t("catalog-start-price")}
            </th>
            <th hidden={!catalogVisibilityContains(props.visibility, "leader")} scope="col">
              {t("catalog-leader")}
            </th>
            <th hidden={!catalogVisibilityContains(props.visibility, "remaining")} scope="col">
              {t("catalog-remaining")}
            </th>
            <th hidden={!catalogVisibilityContains(props.visibility, "status")} scope="col">
              {t("catalog-status")}
            </th>
            <th hidden={!catalogVisibilityContains(props.visibility, "actions")} scope="col">
              {t("catalog-actions")}
            </th>
          </tr>
        </thead>
        <tbody>
          {/* 以 lot_id 作为稳定 key，避免刷新后行状态错位绑定到别的 lot。 */}
          <For each={entries()} keyed={(entry) => entry.key}>
            {(entry) => <PreviewCatalogRow row={entry().row} visibility={props.visibility} />}
          </For>
        </tbody>
      </table>
    </div>
  );
}
