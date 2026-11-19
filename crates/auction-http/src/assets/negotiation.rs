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
    attributes
        .split_ascii_whitespace()
        .find_map(|attribute| attribute.strip_prefix(name)?.strip_circumfix("=\"", '"'))
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AssetEncoding {
    Brotli,
    Identity,
}

/// 只选择实际存在且客户端接受的表示。原文始终存在，`None` 表示两种表示均不可用。
pub(super) fn negotiate_encoding(headers: &HeaderMap, has_brotli: bool) -> Option<AssetEncoding> {
    let (mut brotli, mut identity, mut wildcard) = (None, None, None);
    for value in headers.get_all(header::ACCEPT_ENCODING) {
        // 一个无效项不能遮蔽同一行上其他项的显式拒绝。
        let value = String::from_utf8_lossy(value.as_bytes());
        for item in value.split(',') {
            let mut parts = item.split(';');
            let coding = parts.next().unwrap_or_default().trim_ascii();
            let current = if coding.eq_ignore_ascii_case("br") {
                &mut brotli
            } else if coding.eq_ignore_ascii_case("identity") {
                &mut identity
            } else if coding == "*" {
                &mut wildcard
            } else {
                continue;
            };
            let quality = parts
                .filter_map(|parameter| {
                    let (name, value) = parameter.split_once('=').unwrap_or((parameter, ""));
                    name.trim_ascii()
                        .eq_ignore_ascii_case("q")
                        .then(|| encoding_quality(value.trim_ascii()).unwrap_or_default())
                })
                .min()
                .unwrap_or(1000);
            // 重复项的较高权重不能覆盖拒绝，结果也不能依赖头行的顺序。
            *current = Some(current.unwrap_or(quality).min(quality));
        }
    }

    let accepts_identity = identity.map_or_else(|| wildcard != Some(0), |quality| quality > 0);
    // 原文可接受时沿用旧版选择，只有原文被排除才用通配匹配 Brotli。
    let brotli_quality = if accepts_identity { brotli } else { brotli.or(wildcard) };
    if has_brotli && brotli_quality.is_some_and(|quality| quality > 0) {
        Some(AssetEncoding::Brotli)
    } else if accepts_identity {
        Some(AssetEncoding::Identity)
    } else {
        None
    }
}

/// 权重使用千分数。只接受 HTTP `qvalue` 的十进制写法。
fn encoding_quality(value: &str) -> Option<u16> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let whole = match whole {
        "0" => 0,
        "1" => 1000,
        _ => return None,
    };
    if fraction.len() > 3 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let fractional = fraction
        .bytes()
        .zip([100, 10, 1])
        .map(|(byte, place)| u16::from(byte - b'0') * place)
        .sum::<u16>();
    let quality = whole + fractional;
    (quality <= 1000).then_some(quality)
}

pub(super) fn reserved_json_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/") || path == "/health" || path.starts_with("/health/")
}

#[cfg(test)]
mod tests {
    use axum::http::{HeaderMap, HeaderValue, header};

    use super::{AssetEncoding, negotiate_encoding};

    #[test]
    fn encoding_selection_combines_all_lines_and_available_copies() {
        let br = Some(AssetEncoding::Brotli);
        let identity = Some(AssetEncoding::Identity);
        for (lines, has_brotli, expected) in [
            (&[][..], true, identity),
            (&[""][..], true, identity),
            (&["", " , "], true, identity),
            (&["gzip, br"], true, br),
            (&["gzip, BR;q=0.5"], true, br),
            (&["br;q=0"], true, identity),
            (&["br;Q=0.000"], true, identity),
            (&["gzip, deflate"], true, identity),
            (&["*;q=0.2"], true, identity),
            (&["*;q=0"], true, None),
            (&["*;q=0", "identity;q=0.001"], true, identity),
            (&["*;q=0", "br;q=0.001"], true, br),
            (&["*;q=1", "br;q=0"], true, identity),
            (&["*;q=1", "identity;q=0"], true, br),
            (&["*;q=1", "identity;q=0"], false, None),
            (&["*;q=1", "br;q=0", "identity;q=0"], true, None),
            (&["br;q=0.2, identity;q=0.3"], true, br),
            (&["br;q=0.3, identity;q=0.3"], true, br),
            (&["br;q=0.4, identity;q=0.3"], true, br),
            (&["br;q=0.4, identity;q=0.3"], false, identity),
        ] {
            let mut headers = HeaderMap::new();
            for line in lines {
                headers.append(header::ACCEPT_ENCODING, HeaderValue::from_static(line));
            }
            assert_eq!(negotiate_encoding(&headers, has_brotli), expected, "{lines:?}");
        }
    }

    #[test]
    fn malformed_or_conflicting_weights_cannot_enable_a_representation() {
        for line in [
            "*;q=1, br;q=bad, identity;q=0",
            "*;q=1, br;q=1.001, identity;q=0",
            "*;q=1, br;q=0.0001, identity;q=0",
            "*;q=1, br;q, identity;q=0",
            "*;q=1, br;q=1;q=0, identity;q=0",
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_static(line));
            assert_eq!(negotiate_encoding(&headers, true), None, "{line}");
        }

        let mut headers = HeaderMap::new();
        headers.append(header::ACCEPT_ENCODING, HeaderValue::from_static("br; q = 1"));
        headers.append(
            header::ACCEPT_ENCODING,
            HeaderValue::from_bytes(b"br;q=0, identity;q=0, \xff").unwrap(),
        );
        assert_eq!(negotiate_encoding(&headers, true), None);
    }
}
