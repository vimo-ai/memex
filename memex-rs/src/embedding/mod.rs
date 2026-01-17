//! 文本处理模块
//!
//! 提供文本分片（Chunker）等工具
//!
//! 注意：LLM 能力（Embedding、Chat）已迁移到 `llm` 模块

/// 文本分片器
///
/// 将长文本智能分片，用于向量索引
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
        let paragraphs: Vec<&str> = text
            .split("\n\n")
            .filter(|p| !p.trim().is_empty())
            .collect();

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

/// 文本分片
#[derive(Debug, Clone)]
pub struct Chunk {
    pub index: usize,
    pub content: String,
    pub chunk_type: ChunkType,
}

/// 分片类型
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
