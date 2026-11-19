// 构建环境隔离回归：在每次独立的临时 web 副本里写入 .env，运行真实的 bun run build:public。

import { expect, test } from "bun:test";
import { spawnSync } from "node:child_process";
import {
  cpSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

const WEB_ROOT = fileURLToPath(new URL("..", import.meta.url));
const REPOSITORY_ROOT = resolve(WEB_ROOT, "..");
const OUTPUT = "target/web/public/public";

const ENV_BUILD_ID = "env-build-id-5e2a";
const CALLER_BUILD_ID = "caller-build-id-8f4c";
const ROOT_MARKER = "root-env-marker-1b7d";
const SURFACE_MARKER = "surface-env-marker-6c9e";

/** 准备只够构建 Public 的最小 web 副本；node_modules 与 crates 用符号链接复用。 */
function prepareWorkspace(root: string): void {
  const web = resolve(root, "web");
  mkdirSync(web, { recursive: true });
  writeFileSync(resolve(root, "Cargo.toml"), '[workspace.package]\nversion = "9.9.9"\n');
  symlinkSync(resolve(REPOSITORY_ROOT, "crates"), resolve(root, "crates"), "dir");

  for (const name of ["package.json", "bunfig.toml", "tsconfig.json", "tsconfig.base.json"]) {
    cpSync(resolve(WEB_ROOT, name), resolve(web, name));
  }
  cpSync(resolve(WEB_ROOT, "tooling"), resolve(web, "tooling"), { recursive: true });
  cpSync(resolve(WEB_ROOT, "public"), resolve(web, "public"), { recursive: true });
  for (const name of ["contracts", "shared", "admin"]) {
    symlinkSync(resolve(WEB_ROOT, name), resolve(web, name), "dir");
  }
  symlinkSync(resolve(WEB_ROOT, "node_modules"), resolve(web, "node_modules"), "dir");
}

/** web/ 与 surface 各放一份环境文件；两者都不应进入真实构建产物。 */
function writeEnvFiles(root: string): void {
  const web = resolve(root, "web");
  for (const directory of [web, resolve(web, "public")]) {
    const variables =
      directory === web
        ? `AUCTION_BUILD_ID=${ENV_BUILD_ID}\nVITE_DS3A_ROOT_PROBE=${ROOT_MARKER}\n`
        : `VITE_DS3A_SURFACE_PROBE=${SURFACE_MARKER}\n`;
    writeFileSync(resolve(directory, ".env"), variables);
    writeFileSync(resolve(directory, ".env.production"), variables);
  }
}

/** 只在临时副本的入口里读取两个 VITE_ 标记，制造可观察的泄漏点。 */
function appendEnvProbe(root: string): void {
  const entry = resolve(root, "web/public/src/main.tsx");
  const probe = [
    "globalThis.__ds3aRootProbe = import.meta.env.VITE_DS3A_ROOT_PROBE;",
    "globalThis.__ds3aSurfaceProbe = import.meta.env.VITE_DS3A_SURFACE_PROBE;",
    "",
  ].join("\n");
  writeFileSync(entry, `${readFileSync(entry, "utf8")}\n${probe}`);
}

function runBuild(root: string, overrides: Record<string, string> = {}): string {
  const env: NodeJS.ProcessEnv = { ...process.env };
  delete env.AUCTION_BUILD_ID;
  delete env.VITE_DS3A_ROOT_PROBE;
  delete env.VITE_DS3A_SURFACE_PROBE;
  Object.assign(env, overrides);
  const result = spawnSync("bun", ["run", "build:public"], {
    cwd: resolve(root, "web"),
    env,
    encoding: "utf8",
    timeout: 120_000,
  });
  if (result.error !== undefined || result.status !== 0) {
    throw new Error(`临时构建失败：${result.error?.message ?? result.stderr}`);
  }
  return resolve(root, OUTPUT);
}

function readTree(directory: string): string {
  let text = "";
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = resolve(directory, entry.name);
    text += entry.isDirectory() ? readTree(path) : readFileSync(path, "latin1");
  }
  return text;
}

test("真实构建不读取 web 与 surface 的 .env 文件", () => {
  const root = mkdtempSync(resolve(tmpdir(), "auction-web-build-env-"));
  try {
    prepareWorkspace(root);
    writeEnvFiles(root);
    appendEnvProbe(root);

    const configured = readTree(runBuild(root));
    expect(configured).not.toContain(ENV_BUILD_ID);
    expect(configured).not.toContain(ROOT_MARKER);
    expect(configured).not.toContain(SURFACE_MARKER);

    const caller = readTree(runBuild(root, { AUCTION_BUILD_ID: CALLER_BUILD_ID }));
    expect(caller).toContain(CALLER_BUILD_ID);
    expect(caller).not.toContain(ENV_BUILD_ID);
    expect(caller).not.toContain(ROOT_MARKER);
    expect(caller).not.toContain(SURFACE_MARKER);

    // 对照组：在临时副本里恢复 Bun 与 Vite 的环境文件加载，确认上面的断言能发现泄漏。
    const bunfig = resolve(root, "web/bunfig.toml");
    writeFileSync(bunfig, readFileSync(bunfig, "utf8").replace("env = false", "env = true"));
    const viteConfig = resolve(root, "web/tooling/vite-config.ts");
    writeFileSync(viteConfig, readFileSync(viteConfig, "utf8").replace("    envDir: false,\n", ""));
    const control = readTree(runBuild(root));
    expect(control).toContain(ENV_BUILD_ID);
    expect(control).toContain(ROOT_MARKER);
    expect(control).toContain(SURFACE_MARKER);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
}, 120_000);
