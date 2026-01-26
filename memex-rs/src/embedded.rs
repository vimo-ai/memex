//! 嵌入式 Web 资源模块
//!
//! 使用 rust-embed 在编译时将 web/dist 目录嵌入到 binary 中
//! 支持 MEMEX_WEB_DIR 环境变量覆盖（用于开发）

use axum::{
    body::Body,
    http::{header, Request, Response, StatusCode},
};
use rust_embed::Embed;
use std::path::Path;

/// 嵌入的 Web 资源
///
/// 编译时从 ../web/dist 目录读取所有文件
/// 如果 dist 目录不存在，编译仍然成功，但资源为空
#[derive(Embed)]
#[folder = "../web/dist"]
#[include = "*"]
#[include = "**/*"]
pub struct EmbeddedAssets;

/// 获取嵌入的文件内容和 MIME 类型
///
/// # Arguments
/// * `path` - 文件路径（相对于 web/dist）
///
/// # Returns
/// * `Some((content, mime_type))` - 文件内容和 MIME 类型
/// * `None` - 文件不存在
pub fn get_embedded_file(path: &str) -> Option<(Vec<u8>, String)> {
    // 规范化路径：移除开头的 /
    let path = path.trim_start_matches('/');

    EmbeddedAssets::get(path).map(|file| {
        let content = file.data.to_vec();
        let mime = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();
        (content, mime)
    })
}

/// 检查是否有嵌入的资源
///
/// 用于判断是否应该使用嵌入资源还是外部目录
pub fn has_embedded_assets() -> bool {
    // 检查 index.html 是否存在作为标志
    EmbeddedAssets::get("index.html").is_some()
}

/// 嵌入资源的 SPA 路由处理器
///
/// 支持：
/// - 静态文件服务（带正确的 MIME 类型）
/// - SPA fallback（未知路径返回 index.html）
pub async fn embedded_spa_handler(req: Request<Body>) -> Response<Body> {
    let path = req.uri().path();

    // 尝试获取请求的文件
    if let Some((content, mime)) = get_embedded_file(path) {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, cache_control_for_path(path))
            .body(Body::from(content))
            .unwrap();
    }

    // 检查是否是带扩展名的资源请求（如 .js, .css, .png）
    // 这些请求如果找不到应该返回 404
    if has_extension(path) {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("Not Found"))
            .unwrap();
    }

    // SPA fallback：返回 index.html
    if let Some((content, mime)) = get_embedded_file("index.html") {
        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, mime)
            .header(header::CACHE_CONTROL, "no-cache")
            .body(Body::from(content))
            .unwrap();
    }

    // 没有 index.html，说明嵌入资源不完整
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("Web UI not available"))
        .unwrap()
}

/// 判断路径是否有文件扩展名
fn has_extension(path: &str) -> bool {
    Path::new(path)
        .extension()
        .map(|ext| !ext.is_empty())
        .unwrap_or(false)
}

/// 根据路径返回缓存控制策略
fn cache_control_for_path(path: &str) -> &'static str {
    // 带 hash 的资源文件可以长期缓存
    if path.contains("/assets/") {
        "public, max-age=31536000, immutable"
    } else if path == "/index.html" || path == "/" {
        // HTML 文件不缓存
        "no-cache"
    } else {
        // 其他文件短期缓存
        "public, max-age=3600"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_extension() {
        assert!(has_extension("/assets/index.js"));
        assert!(has_extension("/favicon.ico"));
        assert!(has_extension("style.css"));
        assert!(!has_extension("/projects"));
        assert!(!has_extension("/api/sessions"));
        assert!(!has_extension("/"));
    }

    #[test]
    fn test_cache_control() {
        assert_eq!(
            cache_control_for_path("/assets/index-abc123.js"),
            "public, max-age=31536000, immutable"
        );
        assert_eq!(cache_control_for_path("/index.html"), "no-cache");
        assert_eq!(cache_control_for_path("/"), "no-cache");
        assert_eq!(cache_control_for_path("/favicon.ico"), "public, max-age=3600");
    }
}
