//! Public 列表的统一分页选项、译文和查询状态更新；不改变 Admin 分页策略。

import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import { writeQueryPairs } from "@auction/web-shared/net/query";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { Pager } from "@auction/web-shared/ui/components/pager";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import { type PublicBoardQuery, readPublicQuery, updatePublicQueryState } from "../query";
import { savePublicPageSize } from "./preferences";

export { parsePublicPageSize, storedPublicPageSize } from "./preferences";

export const DEFAULT_PAGE_SIZE = 20;
export const PUBLIC_PAGE_SIZES = [10, 20, 50, 100] as const;

// 预设与默认值都必须是后端接受的页长，否则选择预设就会得到 400。
if (DEFAULT_PAGE_SIZE < 1 || DEFAULT_PAGE_SIZE > MAX_PUBLIC_PAGE_SIZE) {
  throw new Error("Public 默认页长超出契约范围");
}
if (PUBLIC_PAGE_SIZES[3] > MAX_PUBLIC_PAGE_SIZE) {
  throw new Error("Public 分页预设超出契约上限");
}

/** 每页数量只接受 1 到契约上限；超出的页长后端一律拒绝，不能写进 URL、偏好或请求。 */
export function acceptedPublicPageSize(size: number): number | null {
  return Number.isInteger(size) && size >= 1 && size <= MAX_PUBLIC_PAGE_SIZE ? size : null;
}

/** 非法或超过上限的数量回退默认值，不截断成上限。 */
export function normalizePublicPageSize(size: number): number {
  return acceptedPublicPageSize(size) ?? DEFAULT_PAGE_SIZE;
}

/** 切换数量与回到首页作为一次 URL 更新，保留世界、场次和筛选条件。 */
export function updatePublicPageSize(state: Cell<PublicBoardQuery>, size: number): boolean {
  return updatePublicQueryState(
    state,
    (query) => {
      query.pageSize = normalizePublicPageSize(size);
      query.page = 1;
    },
    "push",
  );
}

/** 资源重读期间不能把旧页总数用于新分页状态的纠偏；可选字段沿用旧 DTO 兼容语义。 */
export function publicPageMatches(
  page: number | null | undefined,
  pageSize: number | null | undefined,
  requestedPage: number,
  requestedSize: number,
): boolean {
  return (
    (page == null || page === requestedPage) && (pageSize == null || pageSize === requestedSize)
  );
}

export interface PublicPagerProps {
  readonly page: number;
  readonly pageSize: number;
  readonly total: number | null;
  readonly hasMore?: boolean;
  readonly movement?: boolean;
  readonly maxOffset?: number | null;
  readonly hideWhenSingle?: boolean;
  readonly showSizePicker?: boolean;
  readonly jumpLabel?: string;
  readonly maxPageSize?: number | null;
  readonly label: string;
  readonly onPageChange: (page: number) => void;
  readonly onPageClamp?: (page: number) => void;
  /** 返回 `false` 表示地址栏写入失败；分页组件据此决定是否保存本地偏好。 */
  readonly onPageSizeChange?: (size: number) => unknown;
}

/** 页长偏好只能在页面的地址栏写入成功后保存；回调未提供或没有返回值时沿用旧行为。 */
export function commitPublicPageSizeChoice(
  size: number,
  movement: boolean,
  writePageSize: ((size: number) => unknown) | undefined,
): boolean {
  if (writePageSize?.(size) === false) {
    return false;
  }
  savePublicPageSize(size, movement);
  return true;
}

/** 注入 Public 五语言文案、1 到契约上限的数量与本地偏好，分页计算及键盘交互由共享组件负责。
 * `hideWhenSingle` 保留挂载但隐藏控件；`showSizePicker` 关闭数字页码与数量选择器时保留经典布局。
 */
export function PublicPager(props: PublicPagerProps): JSX.Element {
  const pageSize = createValueMemo(() => Math.max(props.pageSize, 1));
  const maximum = createValueMemo(() =>
    Math.min(props.maxPageSize ?? MAX_PUBLIC_PAGE_SIZE, MAX_PUBLIC_PAGE_SIZE),
  );
  const maxOffset = createValueMemo(() =>
    props.maxOffset === undefined ? 10_000 : props.maxOffset,
  );
  const maxPage = createValueMemo(() => {
    const offset = maxOffset();
    return offset === null ? null : Math.trunc(offset / pageSize()) + 1;
  });
  const movement = createValueMemo(() => props.movement ?? false);
  const total = () => props.total;
  const singlePage = () => {
    const count = total();
    return count !== null && count <= pageSize();
  };
  const pager = (
    <Pager
      page={props.page}
      pageSize={pageSize()}
      total={total()}
      hasMore={props.hasMore ?? false}
      maxPage={maxPage()}
      label={props.label}
      previousLabel={t("catalog-previous")}
      status={(value) => t("catalog-page", { page: value[0], pages: value[1] })}
      nextLabel={t("catalog-next")}
      jumpLabel={props.jumpLabel ?? t("pagination-jump")}
      jumpButtonLabel={t("catalog-page-go")}
      totalSummary={total() === null ? "" : t("pagination-total", { count: total() ?? 0 })}
      rangeSummary={(value) =>
        t("pagination-range", {
          start: String(value[0]),
          end: String(value[1]),
          total: String(value[2]),
        })
      }
      pageSizeOptions={props.showSizePicker === false ? [] : PUBLIC_PAGE_SIZES}
      pageSizeLabel={t("pagination-size")}
      pageSizeOptionLabel={(count) => t("pagination-size-option", { count: String(count) })}
      maxPageSize={maximum()}
      allowCustomPageSize={true}
      pageSizeCustomLabel={t("pagination-custom")}
      pageSizeApplyLabel={t("pagination-apply")}
      pageSizeInvalidLabel={t("pagination-size-limit", { maximum: String(maximum()) })}
      onPageChange={props.onPageChange}
      onPageClamp={props.onPageClamp ?? null}
      onPageSizeChange={(size) => {
        // 共享分页器已拒绝超出上限的数量；这里再守一次，不把它写进地址栏与偏好。
        if (acceptedPublicPageSize(size) === null || size > maximum()) {
          return;
        }
        // 先冻结离开条目的数量（含 10），再更新 URL 和偏好；返回时不读到新偏好。
        // 只替换数量键，不能覆盖出价日志尚在防抖写回的筛选。
        const previous = readPublicQuery();
        writeQueryPairs(
          ["page_size", "movement_page_size"],
          [
            ["page_size", String(previous.pageSize)],
            ["movement_page_size", String(previous.movementPageSize)],
          ],
          "replace",
        );
        return commitPublicPageSizeChoice(size, movement(), props.onPageSizeChange);
      }}
    />
  );
  return (
    <Show when={props.hideWhenSingle} fallback={pager}>
      <div hidden={singlePage()}>{pager}</div>
    </Show>
  );
}
