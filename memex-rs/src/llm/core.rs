//! LLM 共享核心层
//!
//! 提供 HTTP client、配置、错误处理等共享功能

use anyhow::{Context, Result};
use reqwest::Client;
use serde::de::DeserializeOwned;
use std::time::Duration;

/// LLM Client 核心配置
#[derive(Debug, Clone)]
pub struct LlmClientConfig {
    /// API 基础 URL
    pub base_url: String,
    /// API Key（可选，部分 provider 需要）
    pub api_key: Option<String>,
    /// 请求超时时间
    pub timeout: Duration,
}

impl LlmClientConfig {
    /// 创建配置（无 API Key）
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: None,
            timeout: Duration::from_secs(60),
        }
    }

    /// 设置 API Key
    pub fn with_api_key(mut self, api_key: impl Into<String>) -> Self {
        self.api_key = Some(api_key.into());
        self
    }

    /// 设置超时时间
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

/// LLM Client 共享核心
///
/// 封装 HTTP client、鉴权、错误处理等通用逻辑
#[derive(Clone)]
pub struct LlmClientCore {
    client: Client,
    config: LlmClientConfig,
}

impl LlmClientCore {
    /// 创建 LLM Client 核心
    pub fn new(config: LlmClientConfig) -> Self {
        let client = Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("Failed to build HTTP client");

        Self { client, config }
    }

    /// 获取基础 URL
    pub fn base_url(&self) -> &str {
        &self.config.base_url
    }

    /// 获取 API Key
    pub fn api_key(&self) -> Option<&str> {
        self.config.api_key.as_deref()
    }

    /// 发送 POST 请求（JSON）
    pub async fn post_json<T, R>(&self, path: &str, body: &T) -> Result<R>
    where
        T: serde::Serialize + ?Sized,
        R: DeserializeOwned,
    {
        let url = format!("{}{}", self.config.base_url, path);

        let mut request = self.client.post(&url).json(body);

        // 添加鉴权头
        if let Some(api_key) = &self.config.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request
            .send()
            .await
            .with_context(|| format!("请求失败: {}", url))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("API 返回错误 {}: {}", status, text);
        }

        response
            .json()
            .await
            .with_context(|| format!("解析响应失败: {}", url))
    }

    /// 发送 GET 请求
    pub async fn get<R>(&self, path: &str) -> Result<R>
    where
        R: DeserializeOwned,
    {
        let url = format!("{}{}", self.config.base_url, path);

        let mut request = self.client.get(&url);

        if let Some(api_key) = &self.config.api_key {
            request = request.header("Authorization", format!("Bearer {}", api_key));
        }

        let response = request
            .send()
            .await
            .with_context(|| format!("请求失败: {}", url))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("API 返回错误 {}: {}", status, text);
        }

        response
            .json()
            .await
            .with_context(|| format!("解析响应失败: {}", url))
    }

    /// 检查服务是否可用（简单 GET 请求）
    pub async fn health_check(&self, path: &str) -> bool {
        let url = format!("{}{}", self.config.base_url, path);
        self.client.get(&url).send().await.is_ok()
    }
}

impl std::fmt::Debug for LlmClientCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LlmClientCore")
            .field("base_url", &self.config.base_url)
            .field("has_api_key", &self.config.api_key.is_some())
            .finish()
    }
}
