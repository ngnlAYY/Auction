// Public 拍卖目录筛选、分组表格和分页。

import { EQUIPMENT_CATEGORY_CODES } from "@auction/contracts/dto/public/catalog/lot";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, Switch } from "solid-js";

import type { CatalogRow } from "../../../api/model";
import { PublicPager } from "../../pagination";
import { CategoryFilter as DesktopCategoryFilter } from "../../presentation/desktop";
import { CategoryFilter as MobileCategoryFilter } from "../../presentation/mobile";
import { catalogCategoryKeys } from "../../query";
import { CatalogColumnsMenu } from "../columns";
import type { CatalogLayout } from "../columns/state";
import { catalogVisibilityForLayout, catalogVisibilityLoad } from "../columns/state";
import { catalogCategoryKey, catalogCount } from "../presentation";
import { LiveCatalogTable } from "./live";
import { PreviewCatalogTable } from "./preview";

/** 竞价期目录表格与行。 */
export { LiveCatalogRow, LiveCatalogTable } from "./live";
/** 预览期目录表格。 */
export { PreviewCatalogTable } from "./preview";

/** 目录主组件：筛选类型、渲染实时或预览表格并接入分页；列偏好只影响本地显示。 */
export function CatalogTable(props: {
  readonly rows: readonly CatalogRow[];
  readonly projectionKind: string;
  readonly auctionStatus: string;
  readonly categoryCounts: ReadonlyMap<string, number>;
  readonly activeCategory: string;
  readonly activeEquipmentGroup: string;
  readonly categoryFocusRequest: Cell<boolean>;
  readonly levelFiltered: boolean;
  readonly pageNumber: number;
  readonly onCategoryChange: (category: string) => boolean;
  readonly onPageChange: (page: number) => void;
  readonly onPageClamp: (page: number) => void;
  readonly onPageSizeChange: (size: number) => void;
  readonly pageTotal: number | null;
  readonly pageSize: number;
  readonly hasMore: boolean;
  readonly canBid: boolean;
  readonly allowSlow: boolean;
  /** 该场冻结策略的代理默认模式；往期目录拼装的已结算场次为 null。 */
  readonly proxyDefaultMode: string | null;
  readonly sessionReady: boolean;
  readonly authenticated: boolean;
  readonly viewerPlayerId: string | null;
}): JSX.Element {
  const visibility = createCell(catalogVisibilityLoad());
  const indices = createValueMemo(() => {
    const active = new Set(catalogCategoryKeys(props.activeCategory));
    const result: number[] = [];
    props.rows.forEach((row, index) => {
      if (active.size === 0 || active.has(catalogCategoryKey(row.category))) {
        result.push(index);
      }
    });
    return result;
  });
  const isLive = createValueMemo(() => props.projectionKind === "live");
  // 历史成交目录仍使用 live 价格投影。操作列在每个阶段都保留查看入口；出价与标记只在
  // 结算前提供，因此按拍卖阶段单独决定。
  const showCommands = createValueMemo(() => props.auctionStatus !== "settled");
  const layout = createValueMemo((): CatalogLayout => {
    if (!isLive()) {
      return "preview";
    }
    return showCommands() ? "live" : "settled";
  });
  const effectiveVisibility = createValueMemo(() =>
    catalogVisibilityForLayout(visibility(), layout()),
  );
  // 固定八项：全部分类在前，其后按 EQUIPMENT_CATEGORY_CODES 的七个大类顺序排列。
  const counts = createValueMemo(
    (): readonly [number, number, number, number, number, number, number, number] => {
      const count = (category: string) => catalogCount(props.categoryCounts, category, props.rows);
      const [one, two, sta, shd, clo, lig, hea] = EQUIPMENT_CATEGORY_CODES;
      return [
        count("all"),
        count(one[0]),
        count(two[0]),
        count(sta[0]),
        count(shd[0]),
        count(clo[0]),
        count(lig[0]),
        count(hea[0]),
      ];
    },
  );

  return (
    <>
      <div class="public-auction-type-toolbar">
        <DesktopCategoryFilter
          active={props.activeCategory}
          counts={counts()}
          onChange={props.onCategoryChange}
        />
        <MobileCategoryFilter
          active={props.activeCategory}
          counts={counts()}
          focusRequest={props.categoryFocusRequest}
          onChange={props.onCategoryChange}
        />
        {/* 列设置在类型工具栏右端：图标按钮加下拉面板，只改写本地列偏好。 */}
        <CatalogColumnsMenu visibility={visibility} layout={layout()} />
      </div>
      <Switch
        fallback={
          <PreviewCatalogTable
            rows={props.rows}
            indices={indices()}
            visibility={effectiveVisibility()}
          />
        }
      >
        <Match when={indices().length === 0}>
          <div class="ui-async-state" role="status">
            <p class="ui-async-state__title">
              {props.activeEquipmentGroup !== "all" || props.levelFiltered
                ? t("catalog-empty-filtered")
                : catalogCategoryKeys(props.activeCategory).length === 0
                  ? t("catalog-empty-all")
                  : t("catalog-empty-category")}
            </p>
          </div>
        </Match>
        <Match when={isLive()}>
          <LiveCatalogTable
            rows={props.rows}
            indices={indices()}
            visibility={effectiveVisibility()}
            showCommands={showCommands()}
            canBid={props.canBid}
            allowSlow={props.allowSlow}
            proxyDefaultMode={props.proxyDefaultMode}
            sessionReady={props.sessionReady}
            authenticated={props.authenticated}
            viewerPlayerId={props.viewerPlayerId}
          />
        </Match>
      </Switch>
      <PublicPager
        pageLevel
        page={props.pageNumber}
        pageSize={props.pageSize}
        total={props.pageTotal}
        hasMore={props.hasMore}
        label={t("auction-pager-label")}
        onPageChange={props.onPageChange}
        onPageClamp={props.onPageClamp}
        onPageSizeChange={props.onPageSizeChange}
      />
    </>
  );
}
