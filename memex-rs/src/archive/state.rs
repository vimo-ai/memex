//! 归档状态管理

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 归档状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveState {
    /// 最后成功时间
    pub last_success: Option<DateTime<Utc>>,
    /// 当前任务
    pub current_task: Option<CurrentTask>,
    /// 失败记录
    pub failures: Vec<FailureRecord>,
    /// 状态文件路径
    #[serde(skip)]
    state_path: PathBuf,
}

/// 当前任务
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentTask {
    /// 任务类型
    pub task_type: TaskType,
    /// 任务状态
    pub status: TaskStatus,
    /// 开始时间
    pub started_at: DateTime<Utc>,
    /// 目标路径
    pub target_path: PathBuf,
}

/// 任务类型
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    DailyArchive,
    WeeklyMerge,
    MonthlyMerge,
    YearlyMerge,
}

/// 任务状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Compressing,
    Verifying,
    Renaming,
    Cleaning,
    Completed,
    Failed,
}

/// 失败记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailureRecord {
    /// 失败时间
    pub timestamp: DateTime<Utc>,
    /// 任务类型
    pub task_type: TaskType,
    /// 错误信息
    pub error: String,
    /// 重试次数
    pub retry_count: u32,
}

impl ArchiveState {
    /// 加载状态
    pub fn load(archive_dir: &Path) -> Result<Self> {
        let state_path = archive_dir.join(".state.json");

        if state_path.exists() {
            let content = std::fs::read_to_string(&state_path)?;
            let mut state: ArchiveState = serde_json::from_str(&content)?;
            state.state_path = state_path;
            Ok(state)
        } else {
            Ok(Self {
                last_success: None,
                current_task: None,
                failures: Vec::new(),
                state_path,
            })
        }
    }

    /// 保存状态
    pub fn save(&self) -> Result<()> {
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&self.state_path, content)?;
        Ok(())
    }

    /// 开始任务
    pub fn start_task(&mut self, task_type: TaskType, target_path: PathBuf) -> Result<()> {
        self.current_task = Some(CurrentTask {
            task_type,
            status: TaskStatus::Compressing,
            started_at: Utc::now(),
            target_path,
        });
        self.save()
    }

    /// 更新任务状态
    pub fn update_status(&mut self, status: TaskStatus) -> Result<()> {
        if let Some(ref mut task) = self.current_task {
            task.status = status;
        }
        self.save()
    }

    /// 记录成功
    pub fn record_success(&mut self, _archive_path: &Path) -> Result<()> {
        self.last_success = Some(Utc::now());
        self.current_task = None;
        self.save()
    }

    /// 记录失败
    pub fn record_failure(&mut self, task_type: TaskType, error: &str) -> Result<()> {
        // 查找是否有相同类型的失败记录
        let existing = self.failures.iter_mut().find(|f| f.task_type == task_type);

        if let Some(record) = existing {
            record.retry_count += 1;
            record.timestamp = Utc::now();
            record.error = error.to_string();
        } else {
            self.failures.push(FailureRecord {
                timestamp: Utc::now(),
                task_type,
                error: error.to_string(),
                retry_count: 1,
            });
        }

        self.current_task = None;
        self.save()
    }

    /// 清除失败记录
    pub fn clear_failure(&mut self, task_type: &TaskType) -> Result<()> {
        self.failures.retain(|f| &f.task_type != task_type);
        self.save()
    }

    /// 获取失败次数
    pub fn failure_count(&self, task_type: &TaskType) -> u32 {
        self.failures
            .iter()
            .find(|f| &f.task_type == task_type)
            .map(|f| f.retry_count)
            .unwrap_or(0)
    }

    /// 是否有未完成的任务
    pub fn has_pending_task(&self) -> bool {
        self.current_task
            .as_ref()
            .map(|t| t.status != TaskStatus::Completed && t.status != TaskStatus::Failed)
            .unwrap_or(false)
    }

    /// 获取统计信息
    pub fn stats(&self) -> ArchiveStats {
        ArchiveStats {
            last_success: self.last_success,
            pending_task: self.has_pending_task(),
            failure_count: self.failures.len(),
            total_retries: self.failures.iter().map(|f| f.retry_count).sum(),
        }
    }
}

/// 归档统计
#[derive(Debug, Clone, Serialize)]
pub struct ArchiveStats {
    pub last_success: Option<DateTime<Utc>>,
    pub pending_task: bool,
    pub failure_count: usize,
    pub total_retries: u32,
}
