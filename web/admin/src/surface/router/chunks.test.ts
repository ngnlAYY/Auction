// 对照 Admin 生产构建的分块图、入口 HTML 与发布体积口径。

import { expect, test } from "bun:test";
import { mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseJson } from "@auction/web-shared/net/wire";
import { array, boolean, object, optional, parse, record, string } from "valibot";

const WEB = fileURLToPath(new URL("../../../../", import.meta.url));
const MANIFEST = record(
  string(),
  object({
    file: string(),
    isEntry: optional(boolean()),
    imports: optional(array(string())),
    dynamicImports: optional(array(string())),
  }),
);

async function command(args: string[], cwd: string): Promise<string> {
  const child = Bun.spawn(args, {
    cwd,
    env: { ...process.env, NODE_ENV: "production" },
    stdout: "pipe",
    stderr: "pipe",
  });
  const [stdout, stderr, code] = await Promise.all([
    new Response(child.stdout).text(),
    new Response(child.stderr).text(),
    child.exited,
  ]);
  if (code !== 0) throw new Error(`命令退出 ${code}\n${stdout}\n${stderr}`);
  return stdout;
}

const PAYLOAD_SCRIPT = `
set -e
source ci/release/build-identity.sh
verify_solid_web_payload admin "$1/public"
web_entry_references "$1/public/index.html"
web_payload_initial_js_bytes "$1"
`;

async function releasePayload(output: string) {
  const result = await command(
    ["bash", "-c", PAYLOAD_SCRIPT, "admin-payload-test", output],
    resolve(WEB, ".."),
  );
  const lines = result.trim().split("\n");
  const bytes = Number(lines.pop());
  if (!Number.isSafeInteger(bytes) || bytes <= 0) throw new Error("首屏 JS 字节数无效");
  const references = (kind: string) =>
    lines.filter((line) => line.startsWith(`${kind}\t`)).map((line) => line.slice(kind.length + 1));
  return { entry: references("entry"), preloads: references("modulepreload"), bytes };
}

test("Admin 入口只预加载静态依赖，路由分块按需下载，重复引用只计一次", async () => {
  const temp = await mkdtemp(join(tmpdir(), "auction-admin-chunks-"));
  const output = join(temp, "public");
  try {
    await command(
      [
        process.execPath,
        "./node_modules/.bin/vite",
        "build",
        "--config",
        "admin/vite.config.ts",
        "--outDir",
        output,
        "--manifest",
      ],
      WEB,
    );
    const parsed = parseJson(await readFile(join(output, ".vite/manifest.json"), "utf8"));
    if (!parsed.ok) throw new Error("Vite 分块清单无法解析");
    const manifest = parse(MANIFEST, parsed.value);
    const entries = Object.entries(manifest).filter(([, chunk]) => chunk.isEntry === true);
    expect(entries).toHaveLength(1);
    const entry = entries[0];
    if (entry === undefined) throw new Error("Vite 清单缺少入口");
    const staticFiles = new Set<string>();
    const visitStatic = (key: string): void => {
      const chunk = manifest[key];
      if (chunk === undefined) throw new Error(`Vite 清单缺少 ${key}`);
      if (staticFiles.has(chunk.file)) return;
      staticFiles.add(chunk.file);
      for (const dependency of chunk.imports ?? []) visitStatic(dependency);
    };
    visitStatic(entry[0]);
    const entryFile = entry[1].file;

    const routeEntries = [
      "alerts/index",
      "auctions/index",
      "audit/index",
      "config/page",
      "credentials/page",
      "deliveries/detail/index",
      "deliveries/page",
      "lots/page",
      "mail/page",
      "realtime/page",
      "renames",
      "returns/detail",
      "rules/page",
      "settlement/page",
      "status",
      "support/page",
      "users/index",
    ].map((route) => `src/surface/${route}.tsx`);
    const dynamicRoutes = entry[1].dynamicImports ?? [];
    for (const route of routeEntries) expect(dynamicRoutes, route).toContain(route);
    // 退件详情也引用列表的资产格式化函数，构建可将列表输出为按需共享块。
    const returnList = "src/surface/returns/list.tsx";
    const detailImports = manifest["src/surface/returns/detail.tsx"]?.imports ?? [];
    const returnListChunk =
      manifest[returnList] === undefined
        ? dynamicRoutes.find((key) => !routeEntries.includes(key) && detailImports.includes(key))
        : returnList;
    if (returnListChunk === undefined) throw new Error("Vite 清单缺少按需加载的退件列表");
    expect(dynamicRoutes).toContain(returnListChunk);
    for (const key of dynamicRoutes) {
      const chunk = manifest[key];
      if (chunk === undefined) throw new Error(`Vite 清单缺少 ${key}`);
      expect(staticFiles.has(chunk.file), key).toBe(false);
    }

    const original = await releasePayload(temp);
    expect(original.entry).toEqual([`/${entryFile}`]);
    const preloadFiles = [...staticFiles].filter((file) => file !== entryFile);
    expect(new Set(original.preloads)).toEqual(new Set(preloadFiles.map((file) => `/${file}`)));
    const bytes = await Promise.all([...staticFiles].map((file) => stat(join(output, file))));
    expect(original.bytes).toBe(bytes.reduce((total, file) => total + file.size, 0));

    const duplicate = preloadFiles[0];
    if (duplicate === undefined) throw new Error("Vite 入口缺少可重复引用的静态依赖");
    const htmlPath = join(output, "index.html");
    const html = await readFile(htmlPath, "utf8");
    await writeFile(
      htmlPath,
      html.replace(
        "</head>",
        `<link rel="modulepreload" crossorigin href="/${duplicate}">\n</head>`,
      ),
    );
    const repeated = await releasePayload(temp);
    expect(repeated.preloads).toHaveLength(original.preloads.length + 1);
    expect(repeated.bytes).toBe(original.bytes);
  } finally {
    await rm(temp, { recursive: true, force: true });
  }
}, 30_000);
