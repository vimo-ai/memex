//! Embedding 服务 - Ollama 集成

#![allow(dead_code)] // 预留 API: embed_batch, chunk_type

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

/// Ollama 客户端
pub struct OllamaClient {
    client: Client,
    base_url: String,
    embedding_model: String,
    chat_model: String,
}

/// Embedding 请求
#[derive(Serialize)]
struct EmbeddingRequest {
    model: String,
    prompt: String,
}

/// Embedding 响应
#[derive(Deserialize)]
struct EmbeddingResponse {
    embedding: Vec<f32>,
}

/// Chat 请求
#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

/// Chat 响应
#[derive(Deserialize)]
struct ChatResponse {
    message: Option<ChatMessageResponse>,
    eval_count: Option<u64>,
}

#[derive(Deserialize)]
struct ChatMessageResponse {
    content: String,
}

impl OllamaClient {
    /// 创建 Ollama 客户端
    pub fn new(base_url: &str, embedding_model: &str, chat_model: &str) -> Self {
        Self {
            client: Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            embedding_model: embedding_model.to_string(),
            chat_model: chat_model.to_string(),
        }
    }

    /// 检查 Ollama 是否可用
    pub async fn is_available(&self) -> bool {
        let url = format!("{}/api/tags", self.base_url);
        self.client.get(&url).send().await.is_ok()
    }

    /// 检查 embedding 模型是否可用
    pub async fn is_embedding_model_available(&self) -> bool {
        self.check_model(&self.embedding_model).await
    }

    /// 检查 chat 模型是否可用
    pub async fn is_chat_model_available(&self) -> bool {
        self.check_model(&self.chat_model).await
    }

    /// 检查模型是否存在
    async fn check_model(&self, model: &str) -> bool {
        let url = format!("{}/api/tags", self.base_url);
        match self.client.get(&url).send().await {
            Ok(resp) => {
                if let Ok(data) = resp.json::<serde_json::Value>().await {
                    if let Some(models) = data.get("models").and_then(|m| m.as_array()) {
                        return models.iter().any(|m| {
                            m.get("name")
                                .and_then(|n| n.as_str())
                                .map(|n| n.starts_with(model))
                                .unwrap_or(false)
                        });
                    }
                }
                false
            }
            Err(_) => false,
        }
    }

    /// 生成 embedding
    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let url = format!("{}/api/embeddings", self.base_url);

        let request = EmbeddingRequest {
            model: self.embedding_model.clone(),
            prompt: text.to_string(),
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Ollama embedding 请求失败")?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("Ollama 返回错误 {}: {}", status, text);
        }

        let result: EmbeddingResponse = response
            .json()
            .await
            .context("解析 embedding 响应失败")?;

        Ok(result.embedding)
    }

    /// 批量生成 embedding
    pub async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut embeddings = Vec::with_capacity(texts.len());
        for text in texts {
            let embedding = self.embed(text).await?;
            embeddings.push(embedding);
        }
        Ok(embeddings)
    }

    /// Chat 生成
    pub async fn chat(&self, prompt: &str) -> Result<ChatResult> {
        let url = format!("{}/api/chat", self.base_url);

        let request = ChatRequest {
            model: self.chat_model.clone(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
            stream: false,
        };

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Ollama chat 请求失败")?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            anyhow::bail!("Ollama 返回错误 {}: {}", status, text);
        }

        let result: ChatResponse = response
            .json()
            .await
            .context("解析 chat 响应失败")?;

        Ok(ChatResult {
            content: result.message.map(|m| m.content).unwrap_or_default(),
            tokens_used: result.eval_count,
        })
    }
}

/// Chat 结果
pub struct ChatResult {
    pub content: String,
    pub tokens_used: Option<u64>,
}

/// 文本分片
#[derive(Clone)]
pub struct Chunker {
    max_length: usize,
    overlap: usize,
    min_length: usize,
}

impl Default for Chunker {
    fn default() -> Self {
        Self {
            max_length: 2000,
            overlap: 200,
            min_length: 100,
        }
    }
}

impl Chunker {
    /// 对文本进行分片
    pub fn chunk(&self, content: &str) -> Vec<Chunk> {
        // 空内容处理
        if content.trim().is_empty() {
            return vec![Chunk {
                index: 0,
                content: "empty".to_string(),
                chunk_type: ChunkType::Text,
            }];
        }

        // 短文本不分片
        if content.len() <= self.max_length {
            return vec![Chunk {
                index: 0,
                content: content.trim().to_string(),
                chunk_type: ChunkType::Text,
            }];
        }

        // 分离代码块和文本
        let segments = self.separate_code_and_text(content);
        let mut all_chunks = Vec::new();

        for segment in segments {
            match segment.segment_type {
                SegmentType::Code => {
                    if segment.content.len() > self.max_length {
                        let code_chunks = self.split_by_length(&segment.content);
                        for c in code_chunks {
                            if c.len() >= self.min_length {
                                all_chunks.push(Chunk {
                                    index: 0,
                                    content: c,
                                    chunk_type: ChunkType::Code,
                                });
                            }
                        }
                    } else if segment.content.trim().len() >= self.min_length {
                        all_chunks.push(Chunk {
                            index: 0,
                            content: segment.content.trim().to_string(),
                            chunk_type: ChunkType::Code,
                        });
                    }
                }
                SegmentType::Text => {
                    let text_chunks = self.split_text_by_paragraph(&segment.content);
                    all_chunks.extend(text_chunks);
                }
            }
        }

        // 安全回退
        if all_chunks.is_empty() {
            return vec![Chunk {
                index: 0,
                content: content.trim().to_string(),
                chunk_type: ChunkType::Text,
            }];
        }

        // 重新编号
        for (i, chunk) in all_chunks.iter_mut().enumerate() {
            chunk.index = i;
        }

        all_chunks
    }

    fn separate_code_and_text(&self, content: &str) -> Vec<Segment> {
        let mut segments = Vec::new();
        let code_block_regex = regex::Regex::new(r"```[\s\S]*?```").unwrap();

        let mut last_index = 0;
        for mat in code_block_regex.find_iter(content) {
            // 代码块之前的文本
            if mat.start() > last_index {
                let text_before = &content[last_index..mat.start()];
                if !text_before.trim().is_empty() {
                    segments.push(Segment {
                        segment_type: SegmentType::Text,
                        content: text_before.to_string(),
                    });
                }
            }

            // 代码块本身
            segments.push(Segment {
                segment_type: SegmentType::Code,
                content: mat.as_str().to_string(),
            });

            last_index = mat.end();
        }

        // 最后的文本
        if last_index < content.len() {
            let text_after = &content[last_index..];
            if !text_after.trim().is_empty() {
                segments.push(Segment {
                    segment_type: SegmentType::Text,
                    content: text_after.to_string(),
                });
            }
        }

        // 如果没有代码块
        if segments.is_empty() {
            segments.push(Segment {
                segment_type: SegmentType::Text,
                content: content.to_string(),
            });
        }

        segments
    }

    fn split_text_by_paragraph(&self, text: &str) -> Vec<Chunk> {
        let mut chunks = Vec::new();
        let paragraphs: Vec<&str> = text.split("\n\n").filter(|p| !p.trim().is_empty()).collect();

        let mut current_chunk = String::new();

        for para in paragraphs {
            let trimmed = para.trim();

            if trimmed.len() > self.max_length {
                // 保存之前的
                if current_chunk.trim().len() >= self.min_length {
                    chunks.push(Chunk {
                        index: 0,
                        content: current_chunk.trim().to_string(),
                        chunk_type: ChunkType::Text,
                    });
                    current_chunk.clear();
                }

                // 超长段落分割
                for c in self.split_by_length(trimmed) {
                    if c.len() >= self.min_length {
                        chunks.push(Chunk {
                            index: 0,
                            content: c,
                            chunk_type: ChunkType::Text,
                        });
                    }
                }
                continue;
            }

            let combined = if current_chunk.is_empty() {
                trimmed.to_string()
            } else {
                format!("{}\n\n{}", current_chunk, trimmed)
            };

            if combined.len() > self.max_length {
                if current_chunk.trim().len() >= self.min_length {
                    chunks.push(Chunk {
                        index: 0,
                        content: current_chunk.trim().to_string(),
                        chunk_type: ChunkType::Text,
                    });
                }
                current_chunk = trimmed.to_string();
            } else {
                current_chunk = combined;
            }
        }

        // 最后的
        if current_chunk.trim().len() >= self.min_length {
            chunks.push(Chunk {
                index: 0,
                content: current_chunk.trim().to_string(),
                chunk_type: ChunkType::Text,
            });
        }

        chunks
    }

    fn split_by_length(&self, text: &str) -> Vec<String> {
        let mut chunks = Vec::new();
        let mut start = 0;

        while start < text.len() {
            let mut end = (start + self.max_length).min(text.len());

            // 确保 end 在字符边界上
            while end > start && !text.is_char_boundary(end) {
                end -= 1;
            }

            // 尝试在句子边界切分
            if end < text.len() {
                let mut search_start = (start + self.max_length).saturating_sub(100).max(start);
                // 确保 search_start 在字符边界上
                while search_start < end && !text.is_char_boundary(search_start) {
                    search_start += 1;
                }
                let search_text = &text[search_start..end];

                let boundaries = ['\n', '。', '！', '？', '.', '!', '?'];
                let mut best = None;

                for boundary in boundaries {
                    if let Some(idx) = search_text.rfind(boundary) {
                        if best.map(|b| idx > b).unwrap_or(true) {
                            best = Some(idx);
                        }
                    }
                }

                if let Some(idx) = best {
                    let mut new_end = search_start + idx + 1;
                    // 确保新的 end 在字符边界上（边界字符可能是多字节的，如 '。'）
                    while new_end < text.len() && !text.is_char_boundary(new_end) {
                        new_end += 1;
                    }
                    end = new_end;
                }
            }

            let chunk = text[start..end].trim();
            if chunk.len() >= self.min_length {
                chunks.push(chunk.to_string());
            }

            let prev_start = start;
            start = end.saturating_sub(self.overlap);
            // 确保 start 在字符边界上
            while start < text.len() && !text.is_char_boundary(start) {
                start += 1;
            }
            if start <= prev_start {
                start = end;
            }
        }

        chunks
    }
}

/// 分片
#[derive(Debug, Clone)]
pub struct Chunk {
    pub index: usize,
    pub content: String,
    pub chunk_type: ChunkType,
}

#[derive(Debug, Clone, Copy)]
pub enum ChunkType {
    Text,
    Code,
}

struct Segment {
    segment_type: SegmentType,
    content: String,
}

enum SegmentType {
    Text,
    Code,
}
