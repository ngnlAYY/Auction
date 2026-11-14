//! 入口文档预加载声明、资源路径识别与请求协商（方法、brotli、保留 JSON 路径）。

use axum::http::{HeaderMap, HeaderValue, Method, header};

/// 把入口文档引用的样式表与预加载资源写成 `Link` 响应头。
///
/// 入口文档每次都要回源再验证；前置 CDN 可以据这个头在等待源站期间先发 103 Early Hints，
/// 浏览器提前取回带指纹的 WASM、脚本与样式。没有 Early Hints 时它与文档内的 `<link>` 等价，
/// 浏览器按 URL 与请求模式去重，不会重复下载。只转写 `assets/` 下、字符受限的同源路径，
/// 属性沿用文档里的写法，保证预加载与真正的请求匹配。
pub(super) fn entry_preload_links(document: &[u8]) -> Option<HeaderValue> {
    let document = std::str::from_utf8(document).ok()?;
    let mut links = Vec::new();
    let mut rest = document;
    while let Some(start) = rest.find("<link ") {
        let tag = &rest[start + "<link ".len()..];
        let Some(end) = tag.find('>') else {
            break;
        };
        let attributes = &tag[..end];
        rest = &tag[end..];
        let Some(path) = link_attribute(attributes, "href").and_then(fingerprinted_asset_path)
        else {
            continue;
        };
        let destination = match link_attribute(attributes, "rel") {
            Some("stylesheet") => "style",
            Some("preload") => match link_attribute(attributes, "as") {
                Some(destination @ ("script" | "fetch" | "style" | "font")) => destination,
                _ => continue,
            },
            _ => continue,
        };
        let mut link = format!("<{path}>; rel=preload; as={destination}");
        if let Some(media_type) = link_attribute(attributes, "type").filter(|value| {
            value.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"/+-.".contains(&byte))
        }) && destination != "style"
        {
            link.push_str(&format!("; type=\"{media_type}\""));
        }
        if attributes
            .split_ascii_whitespace()
            .any(|attribute| attribute == "crossorigin" || attribute.starts_with("crossorigin="))
        {
            link.push_str("; crossorigin");
        }
        links.push(link);
    }
    if links.is_empty() {
        return None;
    }
    HeaderValue::from_str(&links.join(", ")).ok()
}

/// 读取 `name="value"` 形式的属性值；dx 生成的入口文档只使用双引号。
pub(super) fn link_attribute<'a>(attributes: &'a str, name: &str) -> Option<&'a str> {
    attributes.split_ascii_whitespace().find_map(|attribute| {
        let value = attribute.strip_prefix(name)?.strip_prefix("=\"")?;
        value.strip_suffix('"')
    })
}

/// 只接受 `assets/` 下的带指纹文件，并去掉 dx 写入的 `/./` 前缀。
pub(super) fn fingerprinted_asset_path(href: &str) -> Option<String> {
    let relative = href.strip_prefix("/./").or_else(|| href.strip_prefix('/'))?;
    let name = relative.strip_prefix("assets/")?;
    (!name.is_empty()
        && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte)))
    .then(|| format!("/{relative}"))
}

/// 精确静态分派不能依赖后端规范化路径。RustEmbed 的开发态读取允许符号链接，若先传入
/// `..` 或反斜线，既可能越出资源目录，也可能让路径前缀与实际文件的缓存策略不一致。
pub(super) fn exact_asset_path(path: &str) -> Option<&str> {
    let relative_path = path.strip_prefix('/')?;
    if relative_path.contains('\\')
        || relative_path.ends_with(".br")
        || relative_path
            .split('/')
            .any(|segment| segment.is_empty() || matches!(segment, "." | ".."))
    {
        return None;
    }
    Some(relative_path)
}

pub(super) fn static_method(method: &Method) -> bool {
    matches!(*method, Method::GET | Method::HEAD)
}

/// 解析 Accept-Encoding：任一 `br` 项的 q 值大于 0 即接受；非法 q 值按接受处理（宽容）。
pub(super) fn accepts_brotli(headers: &HeaderMap) -> bool {
    headers.get_all(header::ACCEPT_ENCODING).iter().filter_map(|value| value.to_str().ok()).any(
        |value| {
            value.split(',').any(|item| {
                let mut parts = item.split(';');
                let coding = parts.next().unwrap_or_default().trim();
                if !coding.eq_ignore_ascii_case("br") {
                    return false;
                }
                parts.all(|parameter| {
                    let parameter = parameter.trim();
                    match parameter.strip_prefix("q=").or_else(|| parameter.strip_prefix("Q=")) {
                        Some(weight) => {
                            weight.trim().parse::<f32>().map_or(true, |weight| weight > 0.0)
                        }
                        None => true,
                    }
                })
            })
        },
    )
}

pub(super) fn reserved_json_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/") || path == "/health" || path.starts_with("/health/")
}
