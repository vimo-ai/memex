use anyhow::{Context, Result};
use serde_json::Value;
use std::time::Duration;

use crate::config::Config;

pub struct RemoteSearchClient {
    client: reqwest::Client,
    server: String,
    api_key: String,
}

/// 将 server 返回的 HybridSearchResult 转为 MCP 格式，标记 source=remote
pub fn format_remote_results(results: Vec<Value>, local_sessions: &std::collections::HashSet<String>) -> Vec<Value> {
    results
        .into_iter()
        .filter_map(|r| {
            let sid = r["sessionId"].as_str()?;
            if local_sessions.contains(sid) {
                return None;
            }
            Some(serde_json::json!({
                "session": sid,
                "snippet": r["content"].as_str().or(r["snippet"].as_str()).unwrap_or(""),
                "project": r["projectName"].as_str().unwrap_or(""),
                "score": r["score"].as_f64().unwrap_or(0.0),
                "time": r["timestamp"].as_str().unwrap_or(""),
                "source": "remote"
            }))
        })
        .collect()
}

impl RemoteSearchClient {
    pub fn from_config(config: &Config) -> Option<Self> {
        let server = config.sync_server.as_ref()?;
        let api_key = config.sync_api_key.as_ref()?;

        let mut builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .connect_timeout(Duration::from_secs(3));

        if let Some(ref ca_path) = config.sync_ca_cert {
            let expanded: std::path::PathBuf = if ca_path.starts_with("~/") {
                dirs::home_dir()
                    .map(|h| h.join(&ca_path[2..]))
                    .unwrap_or_else(|| ca_path.into())
            } else {
                ca_path.into()
            };
            match std::fs::read(&expanded) {
                Ok(pem) => match reqwest::Certificate::from_pem(&pem) {
                    Ok(cert) => {
                        builder = builder.add_root_certificate(cert);
                        tracing::info!("🔒 Loaded CA cert: {}", expanded.display());
                    }
                    Err(e) => tracing::warn!("Failed to parse CA cert: {e}"),
                },
                Err(e) => tracing::warn!("Failed to read CA cert {}: {e}", expanded.display()),
            }
        }

        let client = builder.build().ok()?;

        tracing::info!("🔗 Remote search enabled: {server}");
        Some(Self {
            client,
            server: server.trim_end_matches('/').to_string(),
            api_key: api_key.clone(),
        })
    }

    pub async fn search(&self, query: &str, limit: usize, level: &str) -> Vec<Value> {
        match self.do_search(query, limit, level).await {
            Ok(results) => results,
            Err(e) => {
                tracing::warn!("Remote search failed: {e:#}");
                vec![]
            }
        }
    }

    async fn do_search(&self, query: &str, limit: usize, level: &str) -> Result<Vec<Value>> {
        let url = format!("{}/api/search", self.server);
        let resp = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .query(&[
                ("q", query),
                ("limit", &limit.to_string()),
                ("level", level),
            ])
            .send()
            .await
            .context("remote search request failed")?;

        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("remote search returned {status}");
        }

        let body: Value = resp.json().await.context("failed to parse remote response")?;
        let results = body
            .get("results")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_test_config(server: Option<&str>, api_key: Option<&str>) -> Config {
        let mut config = Config::default();
        config.sync_server = server.map(String::from);
        config.sync_api_key = api_key.map(String::from);
        config.sync_ca_cert = None;
        config
    }

    #[test]
    fn from_config_returns_none_without_server() {
        let config = make_test_config(None, Some("key"));
        assert!(RemoteSearchClient::from_config(&config).is_none());
    }

    #[test]
    fn from_config_returns_none_without_api_key() {
        let config = make_test_config(Some("https://example.com"), None);
        assert!(RemoteSearchClient::from_config(&config).is_none());
    }

    #[test]
    fn from_config_returns_some_with_both() {
        let config = make_test_config(Some("https://example.com"), Some("key"));
        assert!(RemoteSearchClient::from_config(&config).is_some());
    }

    #[test]
    fn from_config_strips_trailing_slash() {
        let config = make_test_config(Some("https://example.com/"), Some("key"));
        let client = RemoteSearchClient::from_config(&config).unwrap();
        assert_eq!(client.server, "https://example.com");
    }

    #[tokio::test]
    async fn search_returns_empty_on_unreachable_server() {
        let config = make_test_config(Some("https://127.0.0.1:1"), Some("key"));
        let client = RemoteSearchClient::from_config(&config).unwrap();
        let results = client.search("test", 5, "raw").await;
        assert!(results.is_empty());
    }

    #[test]
    fn format_remote_results_dedup_and_tag() {
        let remote = vec![
            json!({"sessionId": "aaa", "content": "hello", "projectName": "P1", "score": 0.9, "timestamp": "2026-04-22"}),
            json!({"sessionId": "bbb", "content": "world", "projectName": "P2", "score": 0.8, "timestamp": "2026-04-21"}),
            json!({"sessionId": "ccc", "content": "dup", "projectName": "P3", "score": 0.7, "timestamp": "2026-04-20"}),
        ];
        let mut local: std::collections::HashSet<String> = std::collections::HashSet::new();
        local.insert("bbb".to_string());

        let formatted = format_remote_results(remote, &local);
        assert_eq!(formatted.len(), 2);
        assert_eq!(formatted[0]["session"], "aaa");
        assert_eq!(formatted[0]["source"], "remote");
        assert_eq!(formatted[1]["session"], "ccc");
        // bbb 被去重
        assert!(formatted.iter().all(|r| r["session"] != "bbb"));
    }

    #[test]
    fn format_remote_results_skips_missing_session_id() {
        let remote = vec![
            json!({"content": "no session id"}),
        ];
        let local = std::collections::HashSet::new();
        let formatted = format_remote_results(remote, &local);
        assert!(formatted.is_empty());
    }
}
