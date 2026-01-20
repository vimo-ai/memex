//! Compact 配置
//!
//! 支持多层可选开关

use serde::{Deserialize, Serialize};

/// Compact 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompactConfig {
    /// 全局开关：是否启用 Compact 功能
    /// 默认关闭，需要用户显式开启
    pub enabled: bool,

    /// L1: Observations（每个工具调用/操作一个）
    pub l1_observations: bool,

    /// L2: Talk Summary（每轮对话一个）
    /// 注意：L3 开启时 L2 必须开启
    pub l2_talk_summary: bool,

    /// L3: Session Summary（整个会话一个）
    /// 注意：开启 L3 会自动开启 L2（因为 L3 依赖 L2）
    pub l3_session_summary: bool,

    /// L1 优化选项
    pub l1: L1Options,

    /// FTS tokenizer 类型
    /// - "trigram": 支持中英文（默认，子串匹配）
    /// - "unicode61": 仅英文（精确词匹配，索引更小）
    #[serde(default = "default_fts_tokenizer")]
    pub fts_tokenizer: String,

    /// 注入配置（Push 模式）
    #[serde(default)]
    pub inject: InjectConfig,
}

/// 注入模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InjectMode {
    /// 不自动注入，纯 Pull 模式（默认）
    #[default]
    None,
    /// 全量注入（SessionStart 时注入最近 L3 摘要）
    Full,
    /// 组合模式（向量匹配，合并所有 sources 结果）
    Combine,
    /// Fallback 模式（向量匹配，按 sources 顺序尝试，有结果即停）
    Fallback,
}

/// 数据源类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InjectSource {
    /// 原始消息（L0）
    Messages,
    /// 工具调用观察（L1）
    Observations,
    /// 对话摘要（L2）
    Talks,
    /// 会话摘要（L3）
    Sessions,
    /// 所有摘要层（L1+L2+L3 的快捷方式）
    Summaries,
}

impl InjectSource {
    /// 展开 summaries 为具体层级
    pub fn expand(sources: &[InjectSource]) -> Vec<InjectSource> {
        let mut result = Vec::new();
        for src in sources {
            match src {
                InjectSource::Summaries => {
                    result.push(InjectSource::Sessions);
                    result.push(InjectSource::Talks);
                    result.push(InjectSource::Observations);
                }
                other => result.push(*other),
            }
        }
        result
    }
}

/// 向量距离类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VectorDistanceType {
    /// 余弦距离（0-2，默认）
    /// 适用于大多数 embedding 模型（如 bge-m3）
    /// similarity = 1 - distance/2
    #[default]
    Cosine,
    /// 欧氏距离
    /// 距离范围 [0, +∞)，越小越相似
    Euclidean,
    /// 点积距离
    /// 要求向量已归一化
    Dot,
}

/// 注入配置
///
/// 配置示例：
/// ```json
/// // 不注入（默认）
/// {}
///
/// // 全量注入 L3 摘要
/// { "mode": "full", "max_sessions": 10 }
///
/// // 向量匹配，组合多个源
/// { "mode": "combine", "sources": ["summaries", "messages"] }
///
/// // 向量匹配，按顺序 fallback
/// { "mode": "fallback", "sources": ["summaries", "messages"] }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct InjectConfig {
    /// 注入模式
    pub mode: InjectMode,

    /// 数据源（combine/fallback 模式需要）
    /// 可选值: messages, observations, talks, sessions, summaries
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<InjectSource>>,

    /// 最大注入的 session 数量（full 模式）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_sessions: Option<usize>,

    /// 最大注入 token 数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<usize>,

    /// 每个源的最大返回数
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_per_source: Option<usize>,

    /// 向量距离类型（默认 cosine）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance_type: Option<VectorDistanceType>,

    /// 向量相似度阈值（0.0-1.0，越高越严格）
    /// cosine: similarity = 1 - distance/2
    /// l2: 不使用 similarity，直接按 distance 排序
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity_threshold: Option<f32>,

    /// 是否限定当前项目
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_scope: Option<bool>,

    /// 时间窗口（天）：只搜索最近 N 天的历史，0 表示不限制
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_window_days: Option<u32>,

    /// 是否启用时间衰减
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_decay: Option<bool>,

    /// 时间衰减半衰期（天）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_decay_halflife: Option<u32>,
}

impl Default for InjectConfig {
    fn default() -> Self {
        Self {
            mode: InjectMode::None,
            sources: None,
            max_sessions: None,
            max_tokens: None,
            limit_per_source: None,
            distance_type: None,
            similarity_threshold: None,
            project_scope: None,
            time_window_days: None,
            time_decay: None,
            time_decay_halflife: None,
        }
    }
}

impl InjectConfig {
    /// 获取 max_sessions（full 模式默认 10）
    pub fn max_sessions(&self) -> usize {
        self.max_sessions.unwrap_or(10)
    }

    /// 获取 max_tokens（默认 2000）
    pub fn max_tokens(&self) -> usize {
        self.max_tokens.unwrap_or(2000)
    }

    /// 获取 limit_per_source（默认 10）
    pub fn limit_per_source(&self) -> usize {
        self.limit_per_source.unwrap_or(10)
    }

    /// 获取 distance_type（默认 Cosine）
    pub fn distance_type(&self) -> VectorDistanceType {
        self.distance_type.unwrap_or_default()
    }

    /// 获取 similarity_threshold（默认 0.65）
    pub fn similarity_threshold(&self) -> f32 {
        self.similarity_threshold.unwrap_or(0.65)
    }

    /// 获取 project_scope（默认 true）
    pub fn project_scope(&self) -> bool {
        self.project_scope.unwrap_or(true)
    }

    /// 获取 time_window_days（默认 90）
    pub fn time_window_days(&self) -> u32 {
        self.time_window_days.unwrap_or(90)
    }

    /// 获取 time_decay（默认 true）
    pub fn time_decay(&self) -> bool {
        self.time_decay.unwrap_or(true)
    }

    /// 获取 time_decay_halflife（默认 30）
    pub fn time_decay_halflife(&self) -> u32 {
        self.time_decay_halflife.unwrap_or(30)
    }

    /// 获取展开后的 sources
    pub fn expanded_sources(&self) -> Vec<InjectSource> {
        match &self.sources {
            Some(sources) => InjectSource::expand(sources),
            None => vec![], // 用户没配置 sources，返回空
        }
    }

    /// 校验配置
    pub fn validate(&self) -> Result<(), String> {
        match self.mode {
            InjectMode::Combine | InjectMode::Fallback => {
                if self.sources.is_none() || self.sources.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
                    return Err(format!("{:?} 模式需要配置 sources", self.mode));
                }
            }
            _ => {}
        }
        Ok(())
    }
}

/// L1 优化选项
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct L1Options {
    /// 跳过空输出的工具调用
    #[serde(default = "default_true")]
    pub prune_empty: bool,

    /// 合并连续同类操作
    #[serde(default = "default_true")]
    pub merge_consecutive: bool,

    /// 连续 N 个同类操作合并为 1 个
    #[serde(default = "default_merge_threshold")]
    pub merge_threshold: usize,
}

fn default_true() -> bool {
    true
}

fn default_merge_threshold() -> usize {
    3
}

fn default_fts_tokenizer() -> String {
    "trigram".to_string()
}

impl Default for CompactConfig {
    fn default() -> Self {
        // 全局开关默认关闭，通过配置文件 ~/.vimo/memex/config.json 开启
        // 注意：L3 依赖 L2，所以 L3=true 时 L2 也必须为 true
        Self {
            enabled: false,
            l1_observations: false,
            l2_talk_summary: true,  // L3=true 时必须开启 L2
            l3_session_summary: true,
            l1: L1Options::default(),
            fts_tokenizer: default_fts_tokenizer(),
            inject: InjectConfig::default(),
        }
    }
}

impl Default for L1Options {
    fn default() -> Self {
        Self {
            prune_empty: true,
            merge_consecutive: true,
            merge_threshold: 3,
        }
    }
}

impl CompactConfig {
    /// 校验并自动修正配置
    ///
    /// L3 依赖 L2，如果开启 L3 但未开启 L2，自动开启 L2
    pub fn validate(&mut self) {
        if self.l3_session_summary && !self.l2_talk_summary {
            tracing::info!("L3 需要 L2，已自动开启 L2");
            self.l2_talk_summary = true;
        }
    }

    /// 是否需要 LLM（全局开关启用且任意一层开启）
    pub fn needs_llm(&self) -> bool {
        self.enabled && (self.l1_observations || self.l2_talk_summary || self.l3_session_summary)
    }

    /// 极简模式（只用原文搜索）
    pub fn minimal() -> Self {
        Self {
            enabled: false,
            l1_observations: false,
            l2_talk_summary: false,
            l3_session_summary: false,
            l1: L1Options::default(),
            fts_tokenizer: default_fts_tokenizer(),
            inject: InjectConfig::default(),
        }
    }

    /// 完整模式（类 claude-mem）
    pub fn full() -> Self {
        Self {
            enabled: true,
            l1_observations: true,
            l2_talk_summary: true,
            l3_session_summary: true,
            l1: L1Options::default(),
            fts_tokenizer: default_fts_tokenizer(),
            inject: InjectConfig {
                mode: InjectMode::Full,
                ..Default::default()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证 Default 配置一致性
    /// Default 配置：enabled=false（需通过配置文件开启），L3=true 时 L2 也为 true
    #[test]
    fn test_default_config_is_consistent() {
        let config = CompactConfig::default();

        // 默认关闭
        assert!(!config.enabled, "Compact should be disabled by default");
        // Default 配置现在是一致的
        assert!(config.l3_session_summary);
        assert!(config.l2_talk_summary, "L2 should be true when L3 is true in default config");
    }

    /// 验证 validate 仍然可以修复手动构造的不一致配置
    #[test]
    fn test_validate_fixes_manual_inconsistency() {
        let mut config = CompactConfig {
            enabled: true,
            l1_observations: false,
            l2_talk_summary: false,  // 手动设置不一致
            l3_session_summary: true,
            l1: L1Options::default(),
            fts_tokenizer: "trigram".to_string(),
            inject: InjectConfig::default(),
        };
        config.validate();

        // validate 后，L2 应该被自动开启
        assert!(config.l3_session_summary);
        assert!(config.l2_talk_summary, "L2 should be auto-enabled when L3 is true");
    }

    /// 验证反序列化默认值是一致的
    #[test]
    fn test_deserialize_default_is_consistent() {
        // 反序列化空 JSON，使用 Default 值
        let json = "{}";
        let config: CompactConfig = serde_json::from_str(json).unwrap();

        // 反序列化后使用 Default 值，应该是一致的
        assert!(config.l3_session_summary);
        assert!(config.l2_talk_summary, "Default L2 should be true when L3 is true");
    }

    /// 验证用户明确配置不一致时，validate 可以修复
    #[test]
    fn test_deserialize_explicit_inconsistent_needs_validate() {
        // 用户明确配置 L3=true, L2=false（不一致配置）
        let json = r#"{"l3_session_summary": true, "l2_talk_summary": false}"#;
        let mut config: CompactConfig = serde_json::from_str(json).unwrap();

        // 用户明确配置的值会覆盖 Default
        assert!(config.l3_session_summary);
        assert!(!config.l2_talk_summary, "User explicit config should be respected before validate");

        // 调用 validate 修复不一致
        config.validate();
        assert!(config.l2_talk_summary, "L2 should be auto-enabled after validate");
    }

    /// 验证 needs_llm 检查 enabled 字段
    #[test]
    fn test_needs_llm_checks_enabled() {
        let config_disabled = CompactConfig {
            enabled: false,
            l1_observations: true,
            l2_talk_summary: true,
            l3_session_summary: true,
            l1: L1Options::default(),
            fts_tokenizer: "trigram".to_string(),
            inject: InjectConfig::default(),
        };
        assert!(!config_disabled.needs_llm(), "needs_llm should return false when disabled");

        let config_enabled = CompactConfig {
            enabled: true,
            l1_observations: true,
            l2_talk_summary: true,
            l3_session_summary: true,
            l1: L1Options::default(),
            fts_tokenizer: "trigram".to_string(),
            inject: InjectConfig::default(),
        };
        assert!(config_enabled.needs_llm(), "needs_llm should return true when enabled");
    }

    /// 验证 InjectConfig 默认值
    #[test]
    fn test_inject_config_default() {
        let config = InjectConfig::default();
        assert_eq!(config.mode, InjectMode::None);
        assert!(config.sources.is_none());
        assert_eq!(config.max_sessions(), 10);
        assert_eq!(config.max_tokens(), 2000);
        assert_eq!(config.distance_type(), VectorDistanceType::Cosine);
        assert!((config.similarity_threshold() - 0.65).abs() < 0.01);
        assert_eq!(config.time_window_days(), 90);
        assert!(config.time_decay());
    }

    /// 验证 distance_type 配置
    #[test]
    fn test_distance_type_serde() {
        // 默认 cosine
        let json = r#"{"mode": "combine", "sources": ["messages"]}"#;
        let config: InjectConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.distance_type(), VectorDistanceType::Cosine);

        // 显式 cosine
        let json = r#"{"mode": "combine", "sources": ["messages"], "distance_type": "cosine"}"#;
        let config: InjectConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.distance_type(), VectorDistanceType::Cosine);

        // 显式 euclidean
        let json = r#"{"mode": "combine", "sources": ["messages"], "distance_type": "euclidean"}"#;
        let config: InjectConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.distance_type(), VectorDistanceType::Euclidean);

        // 显式 dot
        let json = r#"{"mode": "combine", "sources": ["messages"], "distance_type": "dot"}"#;
        let config: InjectConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.distance_type(), VectorDistanceType::Dot);
    }

    /// 验证 InjectMode 序列化
    #[test]
    fn test_inject_mode_serde() {
        let json = r#"{"mode": "combine", "sources": ["messages"]}"#;
        let config: InjectConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.mode, InjectMode::Combine);
        assert_eq!(config.sources, Some(vec![InjectSource::Messages]));

        let json = r#"{"mode": "fallback", "sources": ["summaries", "messages"]}"#;
        let config: InjectConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.mode, InjectMode::Fallback);
        assert_eq!(config.sources, Some(vec![InjectSource::Summaries, InjectSource::Messages]));

        let json = r#"{"mode": "full"}"#;
        let config: InjectConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.mode, InjectMode::Full);

        let json = r#"{"mode": "none"}"#;
        let config: InjectConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.mode, InjectMode::None);
    }

    /// 验证 sources 展开
    #[test]
    fn test_inject_source_expand() {
        let sources = vec![InjectSource::Summaries, InjectSource::Messages];
        let expanded = InjectSource::expand(&sources);
        assert_eq!(expanded, vec![
            InjectSource::Sessions,
            InjectSource::Talks,
            InjectSource::Observations,
            InjectSource::Messages,
        ]);
    }

    /// 验证配置校验
    #[test]
    fn test_inject_config_validate() {
        // none 模式不需要 sources
        let config = InjectConfig { mode: InjectMode::None, ..Default::default() };
        assert!(config.validate().is_ok());

        // full 模式不需要 sources
        let config = InjectConfig { mode: InjectMode::Full, ..Default::default() };
        assert!(config.validate().is_ok());

        // combine 模式需要 sources
        let config = InjectConfig { mode: InjectMode::Combine, ..Default::default() };
        assert!(config.validate().is_err());

        // combine 模式有 sources 时 OK
        let config = InjectConfig {
            mode: InjectMode::Combine,
            sources: Some(vec![InjectSource::Messages]),
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }
}
