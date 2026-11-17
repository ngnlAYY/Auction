// bun test 的预加载脚本：让测试能导入页面模块里的纯函数。
// 只做两件事：提供构建期常量；把 .tsx 按“JSX 不渲染”转译。这些测试断言的是纯逻辑，
// 组件的 JSX 在这里求值为 null；需要真实渲染的用例不能依赖本脚本。

Object.assign(globalThis, {
  __AUCTION_BUILD_ID__: "test-build",
  __AUCTION_PACKAGE_VERSION__: "0.0.0-test",
  jsx: () => null,
  Fragment: Symbol("solid.fragment"),
});

const transpiler = new Bun.Transpiler({
  loader: "tsx",
  tsconfig: {
    compilerOptions: {
      jsx: "react",
      jsxFactory: "jsx",
      jsxFragmentFactory: "Fragment",
    },
  },
});

Bun.plugin({
  name: "solid-test-tsx",
  setup(build) {
    build.onLoad({ filter: /\.tsx$/ }, async (args) => ({
      contents: transpiler.transformSync(await Bun.file(args.path).text()),
      loader: "js",
    }));
  },
});
