//! Compact 配置
//!
//! 支持多层可选开关

use serde::{Deserialize, Serialize};

/// Compact 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompactConfig {
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

impl Default for CompactConfig {
    fn default() -> Self {
        // 注意：L3 依赖 L2，所以 L3=true 时 L2 也必须为 true
        Self {
            l1_observations: false,
            l2_talk_summary: true,  // L3=true 时必须开启 L2
            l3_session_summary: true,
            l1: L1Options::default(),
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

    /// 是否需要 LLM（任意一层开启）
    pub fn needs_llm(&self) -> bool {
        self.l1_observations || self.l2_talk_summary || self.l3_session_summary
    }

    /// 极简模式（只用原文搜索）
    pub fn minimal() -> Self {
        Self {
            l1_observations: false,
            l2_talk_summary: false,
            l3_session_summary: false,
            l1: L1Options::default(),
        }
    }

    /// 完整模式（类 claude-mem）
    pub fn full() -> Self {
        Self {
            l1_observations: true,
            l2_talk_summary: true,
            l3_session_summary: true,
            l1: L1Options::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证 Default 配置一致性（已修复）
    /// Default 配置：L3=true 时 L2 也为 true
    #[test]
    fn test_default_config_is_consistent() {
        let config = CompactConfig::default();

        // Default 配置现在是一致的
        assert!(config.l3_session_summary);
        assert!(config.l2_talk_summary, "L2 should be true when L3 is true in default config");
    }

    /// 验证 validate 仍然可以修复手动构造的不一致配置
    #[test]
    fn test_validate_fixes_manual_inconsistency() {
        let mut config = CompactConfig {
            l1_observations: false,
            l2_talk_summary: false,  // 手动设置不一致
            l3_session_summary: true,
            l1: L1Options::default(),
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
}
