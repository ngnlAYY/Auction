// 构建期注入的全局常量声明；取值由 web/tooling/vite-config.ts 的 define 提供。

/** 页面构建号：发布构建取 AUCTION_BUILD_ID，未注入时为工作区版本。 */
declare const __AUCTION_BUILD_ID__: string;

/** 工作区版本（根 Cargo.toml 的 workspace.package.version）。 */
declare const __AUCTION_PACKAGE_VERSION__: string;
