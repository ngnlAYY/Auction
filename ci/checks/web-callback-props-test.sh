#!/usr/bin/env bash
# 验证 Web 回调属性门禁的声明边界与行号。

set -euo pipefail
cd "$(dirname "$0")/../.."
checker=$PWD/ci/checks/web-callback-props.pl
fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT

test_failed=0
check_fixture() {
	local name=$1
	shift
	local file="$fixture/$name.rs" status=0 expected_status=0 diagnostic expected_lines='' line
	cat >"$file"
	diagnostic=$(printf '%s\0' "$file" | perl "$checker" 2>&1) || status=$?
	if (($#)); then
		expected_status=1
		for line in "$@"; do
			expected_lines+="$file:$line"$'\n'
		done
	fi
	printf '%s\t预期 %s\t实际 %s\n' "$name" "$expected_status" "$status"
	if [[ $status -ne $expected_status ]] ||
		[[ $expected_status -eq 1 && ${diagnostic#*$'\n'} != "${expected_lines%$'\n'}" ]] ||
		[[ $expected_status -eq 0 && $diagnostic != 'Web 回调属性门禁通过' ]]; then
		printf '%s\n' 'Web 回调属性门禁回归失败，退出码或字段行号不匹配。' "$diagnostic" >&2
		test_failed=1
	fi
}

check_fixture allowed <<'RUST'
// #[component] fn Example(on_click: Option<EventHandler<()>>) {}
/* 外层注释 /* 内层注释 */ #[derive(Props)] struct Demo(Option<Callback>); */
const EXAMPLE: &str = r#"#[component] fn Example(cb: Option<Callback<()>>) {}"#;

#[component]
fn Member(#[props(default)] on_click: EventHandler<()>, label: Option<String>) -> Element {
    let local: Option<Callback<()>> = None;
    let example = "#[derive(Props)] struct Example(Option<EventHandler<()>>);";
}

#[component()]
fn WithDefault(
    #[props(default = { let local: Option<Callback<()>> = None; String::new() })]
    label: String,
) -> Element {}

#[derive(Clone, PartialEq)]
struct StoredOption {
    callback: Option<Callback<()>>,
}

fn helper(callback: Option<EventHandler<()>>) {}

#[derive(Props, Clone, PartialEq)]
struct RequiredCallback(FnWrapper, Callback<(i32, i32), String>);
RUST

check_fixture forbidden 5 15 19 <<'RUST'
#[dioxus::prelude::component]
#[allow(dead_code)]
pub(crate) fn Handler<F: Fn() -> ()>(
    #[props(default)]
    on_click: std::option::Option <
        // 支持跨行及带限定路径的声明。
        dioxus::prelude::EventHandler < (i32, i32) >,
    >,
) -> Element {
    let inner: Option<Callback<()>> = None;
}

#[derive(Clone, dioxus::prelude::Props, PartialEq)]
struct Named<T> where T: Copy {
    label: Option<Callback<(T, T), String>>,
}

#[derive(Props, Clone, PartialEq)]
struct Tuple(pub Option<Callback>);
RUST

check_fixture array-where-props 6 <<'RUST'
#[derive(Props)]
struct ArrayBound<const N: usize>
where
    [u8; N]: Default,
{
    on_x: Option<EventHandler<()>>,
}
RUST

check_fixture raw-component 3 <<'RUST'
#[r#component]
fn RawComponent(
    on_x: Option<EventHandler<()>>,
) -> Element {}
RUST

check_fixture raw-props 3 <<'RUST'
#[derive(r#Props)]
struct RawProps {
    on_x: Option<Callback<()>>,
}
RUST

check_fixture cfg-component 3 <<'RUST'
#[cfg_attr(target_arch = "wasm32", component)]
fn WasmComponent(
    on_x: Option<EventHandler<()>>,
) -> Element {}
RUST

check_fixture cfg-props 3 <<'RUST'
#[cfg_attr(target_arch = "wasm32", derive(Clone, Props))]
struct WasmProps {
    on_x: Option<Callback<()>>,
}
RUST

check_fixture nested-cfg-component 7 <<'RUST'
#[cfg_attr(
    all(target_arch = "wasm32", not(test)),
    allow(dead_code),
    cfg_attr(feature = "web", dioxus::r#component),
)]
fn NestedComponent(
    on_x: Option<EventHandler<()>>,
) -> Element {}
RUST

check_fixture nested-cfg-props 7 <<'RUST'
#[cfg_attr(
    target_arch = "wasm32",
    cfg_attr(feature = "web", derive(Clone, dioxus::prelude::r#Props)),
    allow(dead_code),
)]
struct NestedProps {
    on_x: Option<Callback<()>>,
}
RUST

check_fixture component-suffix <<'RUST'
#[my_component]
fn OtherComponent(on_x: Option<EventHandler<()>>) {}
RUST

check_fixture unrelated-cfg-attr <<'RUST'
#[cfg_attr(test, allow(dead_code))]
fn Unrelated(on_x: Option<EventHandler<()>>) {}

#[cfg_attr(component, cfg_attr(test, allow(component)))]
fn PredicateNamedComponent(on_x: Option<EventHandler<()>>) {}

#[cfg_attr(test, derive(Clone, MyProps))]
struct UnrelatedDerive {
    on_x: Option<EventHandler<()>>,
}
RUST

check_fixture ordinary-function <<'RUST'
fn helper(on_x: Option<EventHandler<()>>) {}
RUST

check_fixture ordinary-struct <<'RUST'
#[derive(Clone, PartialEq)]
struct StoredHandler {
    on_x: Option<EventHandler<()>>,
}
RUST

((test_failed == 0)) || exit 1
printf '%s\n' 'Web 回调属性门禁回归通过'
