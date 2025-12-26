//! 备份服务 - 每日增量备份

use anyhow::{Context, Result};
use chrono::Local;
use std::path::PathBuf;

/// 备份服务
#[derive(Clone)]
pub struct BackupService {
    db_path: PathBuf,
    backup_dir: PathBuf,
    max_backups: usize,
}

/// 备份结果
#[derive(Debug, Clone, serde::Serialize)]
pub struct BackupResult {
    pub path: PathBuf,
    pub size: u64,
    pub timestamp: String,
}

/// 备份信息
#[derive(Debug, Clone, serde::Serialize)]
pub struct BackupInfo {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub date: String,
}

impl BackupService {
    /// 创建备份服务
    pub fn new(db_path: PathBuf, backup_dir: PathBuf) -> Self {
        Self {
            db_path,
            backup_dir,
            max_backups: 30, // 保留 30 天备份
        }
    }

    /// 设置最大备份数量
    pub fn with_max_backups(mut self, max: usize) -> Self {
        self.max_backups = max;
        self
    }

    /// 执行备份
    pub fn backup(&self) -> Result<BackupResult> {
        // 确保备份目录存在
        std::fs::create_dir_all(&self.backup_dir)?;

        // 生成备份文件名
        let timestamp = Local::now().format("%Y%m%d%H%M%S").to_string();
        let backup_name = format!("memex.db.bak-{}", timestamp);
        let backup_path = self.backup_dir.join(&backup_name);

        // 检查今天是否已备份
        if self.has_backup_today()? {
            tracing::info!("今日已有备份，跳过");
            // 返回今日最新备份
            if let Some(latest) = self.get_latest_backup()? {
                return Ok(BackupResult {
                    path: latest.path,
                    size: latest.size,
                    timestamp: latest.date,
                });
            }
        }

        // 复制数据库文件
        std::fs::copy(&self.db_path, &backup_path)
            .with_context(|| format!("备份失败: {:?} -> {:?}", self.db_path, backup_path))?;

        let metadata = std::fs::metadata(&backup_path)?;
        let size = metadata.len();

        tracing::info!("备份完成: {:?} ({} bytes)", backup_path, size);

        // 清理旧备份
        self.cleanup_old_backups()?;

        Ok(BackupResult {
            path: backup_path,
            size,
            timestamp,
        })
    }

    /// 检查今天是否已有备份
    fn has_backup_today(&self) -> Result<bool> {
        let today = Local::now().format("%Y%m%d").to_string();
        let backups = self.list_backups()?;

        Ok(backups.iter().any(|b| b.name.contains(&today)))
    }

    /// 获取最新备份
    fn get_latest_backup(&self) -> Result<Option<BackupInfo>> {
        let mut backups = self.list_backups()?;
        backups.sort_by(|a, b| b.name.cmp(&a.name));
        Ok(backups.into_iter().next())
    }

    /// 列出所有备份
    pub fn list_backups(&self) -> Result<Vec<BackupInfo>> {
        if !self.backup_dir.exists() {
            return Ok(vec![]);
        }

        let mut backups = Vec::new();

        for entry in std::fs::read_dir(&self.backup_dir)? {
            let entry = entry?;
            let path = entry.path();

            if !path.is_file() {
                continue;
            }

            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();

            if !name.starts_with("memex.db.bak-") {
                continue;
            }

            let metadata = std::fs::metadata(&path)?;

            // 解析日期
            let date = name
                .strip_prefix("memex.db.bak-")
                .and_then(|s| s.get(0..8))
                .map(|s| format!("{}-{}-{}", &s[0..4], &s[4..6], &s[6..8]))
                .unwrap_or_default();

            backups.push(BackupInfo {
                name,
                path,
                size: metadata.len(),
                date,
            });
        }

        // 按名称降序排列（最新在前）
        backups.sort_by(|a, b| b.name.cmp(&a.name));

        Ok(backups)
    }

    /// 清理旧备份
    fn cleanup_old_backups(&self) -> Result<()> {
        let mut backups = self.list_backups()?;

        if backups.len() <= self.max_backups {
            return Ok(());
        }

        // 按名称排序（最新在前）
        backups.sort_by(|a, b| b.name.cmp(&a.name));

        // 删除超出数量的旧备份
        for backup in backups.into_iter().skip(self.max_backups) {
            tracing::info!("删除旧备份: {:?}", backup.path);
            if let Err(e) = std::fs::remove_file(&backup.path) {
                tracing::warn!("删除备份失败: {:?}: {}", backup.path, e);
            }
        }

        Ok(())
    }

    /// 恢复备份
    pub fn restore(&self, backup_name: &str) -> Result<()> {
        let backup_path = self.backup_dir.join(backup_name);

        if !backup_path.exists() {
            anyhow::bail!("备份文件不存在: {:?}", backup_path);
        }

        // 创建当前数据库的临时备份
        let temp_backup = self.db_path.with_extension("db.restore-backup");
        std::fs::copy(&self.db_path, &temp_backup)?;

        // 恢复备份
        match std::fs::copy(&backup_path, &self.db_path) {
            Ok(_) => {
                // 删除临时备份
                let _ = std::fs::remove_file(&temp_backup);
                tracing::info!("恢复完成: {:?}", backup_path);
                Ok(())
            }
            Err(e) => {
                // 恢复失败，还原临时备份
                let _ = std::fs::copy(&temp_backup, &self.db_path);
                let _ = std::fs::remove_file(&temp_backup);
                Err(e.into())
            }
        }
    }
}
