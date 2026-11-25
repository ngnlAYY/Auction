// 锁定两端全部路由模式在 Rust 批准清单与 Solid 路由表里同集合（仅显式登记的差异除外），
// 并要求 Solid 路由表里名称或路径含寄售字样的条目都在 `/consignment` 前缀下。
// 前缀路由一律走独立入口（裁定 E6）：旧壳没有寄售页面，任一侧漂移都会让回切旧壳后的入口分类出错。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const repositoryRoot = resolve(import.meta.dir, "../..");
const CONSIGNMENT_PREFIX = "/consignment";

function read(relativePath: string): string {
  return readFileSync(resolve(repositoryRoot, relativePath), "utf8");
}

/** 取非测试部分，避免测试里的样例路径混入清单。 */
function productionPart(source: string): string {
  const cut = source.indexOf("#[cfg(test)]");
  return cut === -1 ? source : source.slice(0, cut);
}

function between(source: string, start: string, end: string | null): string {
  const from = source.indexOf(start);
  if (from === -1) {
    throw new Error(`routes.rs 缺少锚点 ${start}`);
  }
  const to = end === null ? source.length : source.indexOf(end, from + start.length);
  if (to === -1) {
    throw new Error(`routes.rs 缺少锚点 ${end}`);
  }
  return source.slice(from, to);
}

function quotedLiterals(block: string): string[] {
  return [...block.matchAll(/"(\/[^"]*)"/g)].map((match) => match[1] ?? "");
}

/** 把 Rust 的 strip_prefix/strip_suffix/strip_circumfix 写法还原成路由表的 `:param` 形状。 */
function patternsFromPrefixCode(block: string): string[] {
  const patterns: string[] = [];
  const withSuffix =
    /strip_suffix\("([^"]+)"\)\s*\.and_then\(\|prefix\| prefix\.strip_prefix\("([^"]+)"\)\)/g;
  for (const match of block.matchAll(withSuffix)) {
    patterns.push(`${match[2]}:${match[1]}`);
  }
  const rest = block.replace(withSuffix, "");
  for (const match of rest.matchAll(/strip_circumfix\("([^"]+)",\s*"([^"]+)"\)/g)) {
    patterns.push(`${match[1]}:${match[2]}`);
  }
  for (const match of rest.matchAll(/strip_prefix\("(\/[^"]*)"\)/g)) {
    patterns.push(`${match[1]}:`);
  }
  return patterns;
}

function rustRoutes(): { readonly publicRoutes: string[]; readonly adminRoutes: string[] } {
  const source = productionPart(read("crates/auction-http/src/assets/routes.rs"));
  const publicExact = between(source, "const PUBLIC_EXACT_ROUTES", "const ADMIN_EXACT_ROUTES");
  const adminExact = between(source, "const ADMIN_EXACT_ROUTES", "#[derive");
  const matcher = between(source, "pub(super) fn browser_route", "fn valid_parameter");
  const shared = between(matcher, "pub(super) fn browser_route", "match surface");
  const publicArm = between(matcher, "Surface::Public =>", "Surface::Admin =>");
  const adminArm = between(matcher, "Surface::Admin =>", null);
  return {
    publicRoutes: [
      ...quotedLiterals(publicExact),
      ...patternsFromPrefixCode(shared),
      ...patternsFromPrefixCode(publicArm),
    ].sort(),
    adminRoutes: [
      ...quotedLiterals(adminExact),
      ...patternsFromPrefixCode(shared),
      ...patternsFromPrefixCode(adminArm),
    ].sort(),
  };
}

/** Solid 路由表里的全部条目；`:name` 一律折成 `:`。 */
function solidEntries(relativePath: string): { name: string; pattern: string }[] {
  const source = read(relativePath);
  return [...source.matchAll(/name:\s*"([^"]+)",\s*pattern:\s*"([^"]+)"/g)].map((match) => ({
    name: match[1] ?? "",
    pattern: (match[2] ?? "").replace(/:[a-z_]+/g, ":"),
  }));
}

function solidRoutes(relativePath: string): string[] {
  return solidEntries(relativePath)
    .map((entry) => entry.pattern)
    .sort();
}

/** Solid 路由表有而 routes.rs 没有的模式：`/setup` 是分体部署首次设置页，由 auction-setup 独立提供入口。 */
const SOLID_ONLY_ADMIN_ROUTES = ["/setup"];

test("browser_routes_match_between_routes_rs_and_solid_tables：两端全部路由两侧同集合", () => {
  const rust = rustRoutes();
  expect(rust.publicRoutes.length).toBeGreaterThan(0);
  expect(rust.adminRoutes.length).toBeGreaterThan(0);
  expect(solidRoutes("web/public/src/surface/router/route.ts")).toEqual(rust.publicRoutes);
  expect(solidRoutes("web/admin/src/surface/router/route.ts")).toEqual(
    [...rust.adminRoutes, ...SOLID_ONLY_ADMIN_ROUTES].sort(),
  );
});

test("solid_consignment_entries_live_under_the_independent_prefix：名称或路径含寄售字样的条目在前缀下", () => {
  for (const table of [
    "web/public/src/surface/router/route.ts",
    "web/admin/src/surface/router/route.ts",
  ]) {
    const entries = solidEntries(table).filter(
      (entry) => /consignment/i.test(entry.name) || /consignment/i.test(entry.pattern),
    );
    expect(entries.length, table).toBeGreaterThan(0);
    for (const entry of entries) {
      expect(
        entry.pattern === CONSIGNMENT_PREFIX || entry.pattern.startsWith(`${CONSIGNMENT_PREFIX}/`),
        `${table}: ${entry.name} ${entry.pattern}`,
      ).toBe(true);
    }
  }
});

test("consignment_prefix_extraction_detects_drift：漂移样例会被识别", () => {
  const drifted = patternsFromPrefixCode(
    'x.strip_prefix("/consignment/listings/") y.strip_prefix("/consignment/orders/")',
  );
  expect(drifted).toEqual(["/consignment/listings/:", "/consignment/orders/:"]);
  expect(drifted).not.toEqual(["/consignment/listings/:"]);
});
