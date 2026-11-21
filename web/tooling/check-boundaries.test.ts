// 覆盖说明符闭集与跨包判定；不启动 TypeScript 解析器，因此可在受限沙箱里运行。

import { describe, expect, test } from "bun:test";
import { resolve } from "node:path";
import {
  classifySpecifier,
  isNamedVirtualModuleFile,
  isTestFile,
  type SpecifierDecision,
} from "./boundary-rules";

const repositoryRoot = "/auction";

function decide(
  specifier: string,
  sourceFile = "web/public/src/example.ts",
  dependencies = ["solid-js", "@solidjs/web", "valibot"],
): SpecifierDecision {
  return classifySpecifier({
    sourceFile: resolve(repositoryRoot, sourceFile),
    specifier,
    repositoryRoot,
    dependencies,
  });
}

function expectViolation(specifier: string, sourceFile?: string): string {
  const result = decide(specifier, sourceFile);
  expect(result.kind).toBe("violation");
  return result.kind === "violation" ? result.reason : "";
}

describe("说明符闭集", () => {
  test("允许包内相对路径", () => {
    expect(decide("./helper").kind).toBe("ok");
    expect(decide("../catalog/helper", "web/public/src/surface/example.ts").kind).toBe("ok");
  });

  test("拒绝反斜杠", () => {
    expect(expectViolation("./..\\../public/src/main")).toContain("反斜杠");
  });

  test("目录入口的相对导入按目标路径判定", () => {
    expect(decide(".", "web/public/src/surface/example.ts").kind).toBe("ok");
    expect(decide("..", "web/public/src/surface/watches/index.tsx").kind).toBe("ok");
    // 从包的 src 根再往上一层就离开了这个包。
    expect(decide("..", "web/public/src/main.tsx").kind).toBe("violation");
  });

  test("允许两个登记别名", () => {
    expect(decide("@auction/contracts/dto/response").kind).toBe("ok");
    expect(decide("@auction/web-shared/result").kind).toBe("ok");
  });

  test("允许 package.json 登记的第三方依赖及子路径", () => {
    expect(decide("solid-js").kind).toBe("ok");
    expect(decide("solid-js/web").kind).toBe("ok");
    expect(decide("@solidjs/web/jsx-runtime").kind).toBe("ok");
  });

  test("第三方包子路径不能借 .. 穿越到 Web 包", () => {
    expect(expectViolation("solid-js/../../admin/src/main")).toContain("第三方包子路径");
    expect(expectViolation("@solidjs/web/../public/src/main")).toContain("第三方包子路径");
  });

  test("内置模块只允许测试文件", () => {
    expect(decide("bun:test", "web/public/src/example.test.ts").kind).toBe("ok");
    expect(decide("node:fs", "web/public/src/example.test.ts").kind).toBe("ok");
    expect(decide("bun", "web/public/src/example.test.ts").kind).toBe("ok");
    expect(expectViolation("bun:test", "web/public/src/example.ts")).toContain("测试文件");
  });

  test("拒绝未登记裸包与别名", () => {
    expect(expectViolation("left-pad")).toContain("未登记");
    expect(expectViolation("@auction/web-public/helper")).toContain("未登记");
  });

  test("拒绝绝对路径、协议 URL 与包内导入", () => {
    expect(expectViolation("/auction/web/public/src/main")).toContain("绝对路径");
    expect(expectViolation("file:///auction/web/public/src/main")).toContain("URL");
    expect(expectViolation("http://example.test/module")).toContain("URL");
    expect(expectViolation("data:text/javascript,export{}")).toContain("URL");
    expect(expectViolation("#internal")).toContain("包内导入");
  });

  test("按 AST 解码后的 Unicode 文本判定目标", () => {
    expect(expectViolation("../../\u0061dmin/src/main")).toContain("跨包导入");
  });
});

describe("包方向与目标归属", () => {
  test("相对路径跨包一律拒绝", () => {
    expect(expectViolation("../../shared/src/helper")).toContain("跨包导入");
    expect(expectViolation("../../admin/src/main", "web/public/src/example.ts")).toContain(
      "跨包导入",
    );
  });

  test("别名按目标归属判定，不能从 contracts 跳到 public", () => {
    expect(
      expectViolation("@auction/contracts/../../public/src/main", "web/contracts/src/a.ts"),
    ).toContain("未登记的 @auction 包名");
  });

  test("shared 只能导入 contracts 与自身", () => {
    expect(decide("@auction/contracts/dto/response", "web/shared/src/a.ts").kind).toBe("ok");
    expect(decide("@auction/web-shared/result", "web/shared/src/a.ts").kind).toBe("ok");
    expect(
      expectViolation("@auction/web-shared/../../public/src/main", "web/contracts/src/a.ts"),
    ).toContain("未登记的 @auction 包名");
  });

  test("contracts 不导入 shared/public/admin", () => {
    for (const target of ["shared", "public", "admin"]) {
      expect(expectViolation(`../../${target}/src/main`, "web/contracts/src/a.ts")).toContain(
        "跨包导入",
      );
    }
  });

  test("public 与 admin 互不导入", () => {
    expect(expectViolation("../../admin/src/main", "web/public/src/a.ts")).toContain("跨包导入");
    expect(expectViolation("../../public/src/main", "web/admin/src/a.ts")).toContain("跨包导入");
  });

  test("不能导入 web/tooling 或四个包 src 之外的源码", () => {
    expect(expectViolation("../../tooling/test-runtime", "web/shared/src/a.ts")).toContain(
      "web/tooling",
    );
    expect(expectViolation("../../vite.config.ts", "web/public/src/a.ts")).toContain(
      "四个包的 src",
    );
  });

  test("资产按归属放行", () => {
    expect(decide("../../../crates/auction-web-shared/style/base.css").kind).toBe("ok");
    expect(decide("../../../crates/auction-web-public/i18n/en.ftl?raw").kind).toBe("ok");
    expect(
      decide("../../../crates/auction-web-admin/assets/admin.css", "web/admin/src/a.ts").kind,
    ).toBe("ok");
    expect(expectViolation("../../../crates/auction-web-admin/assets/admin.css")).toContain(
      "web/ 之外",
    );
    expect(expectViolation("../../../crates/auction-web-admin/i18n/en.ftl?raw")).toContain(
      "web/ 之外",
    );
    expect(expectViolation("../../../crates/auction-web-shared/assets/favicon.svg")).toContain(
      "web/ 之外",
    );
  });

  test("在配置中登记的依赖子路径仍受包方向约束，第三方路径放行", () => {
    expect(decide("valibot/parse").kind).toBe("ok");
    expect(expectViolation("unknown-package/subpath")).toContain("未登记");
  });

  test("别名穿越到包外或 web/ 之外也被拒绝", () => {
    expect(
      expectViolation("@auction/contracts/../../../web/public/src/main", "web/contracts/src/a.ts"),
    ).toContain("未登记的 @auction 包名");
    expect(
      expectViolation("@auction/contracts/../../../../crates/auction-web-public/src/main"),
    ).toContain("未登记的 @auction 包名");
  });

  test("测试虚拟模块例外按完整文件路径点名", () => {
    expect(
      isNamedVirtualModuleFile(
        resolve(repositoryRoot, "web/public/src/surface/shell/navigation-fixture.test.ts"),
        repositoryRoot,
      ),
    ).toBe(true);
    expect(
      isNamedVirtualModuleFile(
        resolve(repositoryRoot, "web/public/src/surface/catalog/command-scope-fixture.test.ts"),
        repositoryRoot,
      ),
    ).toBe(true);
    expect(
      isNamedVirtualModuleFile(
        resolve(repositoryRoot, "web/public/src/other.test.ts"),
        repositoryRoot,
      ),
    ).toBe(false);
    expect(
      isTestFile(resolve(repositoryRoot, "web/public/src/other.test.ts"), repositoryRoot),
    ).toBe(true);
    expect(isTestFile(resolve(repositoryRoot, "web/public/src/other.ts"), repositoryRoot)).toBe(
      false,
    );
  });
});
