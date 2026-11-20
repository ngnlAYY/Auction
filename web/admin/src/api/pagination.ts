// 浏览器请求与响应分页边界；只校验显式参数，不读取展示偏好或截断用户请求。

import { err, ok, type Result } from "@auction/web-shared/result";

import { MAX_JSON_SAFE_INTEGER } from "./validation";

/** Rust `i64` 的上界；请求偏移仍须落在 Rust 会接受的范围内。 */
const I64_MAX = 9_223_372_036_854_775_807n;

/** 页码分页请求的三项约束：正页码、正页大小且不超过端点上限、起始偏移不超过最大 offset。
 * Rust 的 `checked_mul` 在乘积越界时落回同一错误；页码与页大小仍只接受安全整数，
 * 偏移用 bigint 精确计算，避免把合法的 `i64` 偏移误判为越界。
 * `maxOffset` 可传 bigint 以精确表达 `i64::MAX`。 */
export function validate(
  page: number,
  size: number,
  maximum: number,
  maxOffset: number | bigint,
): Result<null, string> {
  if (
    !Number.isSafeInteger(page) ||
    !Number.isSafeInteger(size) ||
    page <= 0 ||
    size <= 0 ||
    size > maximum
  ) {
    return err("admin_pagination_bounds");
  }
  const offset = (BigInt(page) - 1n) * BigInt(size);
  const maximumOffset =
    typeof maxOffset === "bigint"
      ? maxOffset
      : Number.isInteger(maxOffset)
        ? BigInt(maxOffset)
        : null;
  if (maximumOffset === null || offset > maximumOffset || offset > I64_MAX) {
    return err("admin_pagination_bounds");
  }
  return ok(null);
}

/** 响应分页三元组均须是 JSON 安全整数：页码与页大小为正，总数非负。 */
export function validResponse(page: number, pageSize: number, total: number): boolean {
  return (
    Number.isSafeInteger(page) &&
    page >= 1 &&
    page <= MAX_JSON_SAFE_INTEGER &&
    Number.isSafeInteger(pageSize) &&
    pageSize >= 1 &&
    pageSize <= MAX_JSON_SAFE_INTEGER &&
    Number.isSafeInteger(total) &&
    total >= 0 &&
    total <= MAX_JSON_SAFE_INTEGER
  );
}

/**
 * 页码分页响应的回显校验：三元组本身合法，页码与页大小原样回显请求，本页条数不超过页大小
 * 与总数。页大小上限、分类计数、游标与筛选回显等端点特有约束由调用方另行校验。
 */
export function validPageEcho(
  requestedPage: number,
  requestedSize: number,
  page: number,
  pageSize: number,
  total: number,
  itemCount: number,
): boolean {
  return (
    validResponse(page, pageSize, total) &&
    page === requestedPage &&
    pageSize === requestedSize &&
    Number.isSafeInteger(itemCount) &&
    itemCount >= 0 &&
    itemCount <= pageSize &&
    itemCount <= total
  );
}
