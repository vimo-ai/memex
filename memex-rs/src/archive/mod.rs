//! 归档服务 - JSONL 渐进式压缩归档
//!
//! 策略：日包 → 周包 → 月包 → 年包
//! 压缩：xz -9e -T0（极致压缩，多线程）
//! 校验：SHA-256

mod compressor;
mod state;

pub use compressor::Compressor;
pub use state::{ArchiveState, TaskStatus};

use anyhow::Result;
use chrono::{Datelike, Local, NaiveDate};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// 归档服务
pub struct ArchiveService {
    /// CC 项目目录
    source_dir: PathBuf,
    /// 归档目录
    archive_dir: PathBuf,
    /// 压缩器
    compressor: Compressor,
    /// 状态管理
    state: ArchiveState,
    /// 文件静默时间（秒），默认 24 小时
    quiet_period: u64,
}

/// 归档结果
#[derive(Debug, Clone, serde::Serialize)]
pub struct ArchiveResult {
    pub archive_path: PathBuf,
    pub files_count: usize,
    pub original_size: u64,
    pub compressed_size: u64,
    pub compression_ratio: f64,
}

/// 全量归档检查结果
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ArchiveAllResult {
    /// 日归档数量
    pub daily_archived: usize,
    /// 归档的文件总数
    pub files_archived: usize,
    /// 周合并数量
    pub weekly_merged: usize,
    /// 月合并数量
    pub monthly_merged: usize,
    /// 年合并数量
    pub yearly_merged: usize,
    /// 错误列表
    pub errors: Vec<String>,
}

/// 可归档的会话单元（主文件 + 所有 subagent 作为原子单位）
#[allow(dead_code)]
struct ArchivableSession {
    /// 会话 UUID（主文件或目录名推导）
    uuid: String,
    /// 主 session 文件的相对路径（从 source_dir 起算）
    /// None = 孤立 subagent（主文件已被清理）
    main_file: Option<PathBuf>,
    /// subagent 文件的相对路径列表
    subagent_files: Vec<PathBuf>,
    /// effective mtime = max(main, all subagents)
    effective_mtime: SystemTime,
}

impl ArchivableSession {
    /// 所有文件的相对路径（main + subagents）
    fn all_relative_paths(&self) -> Vec<&Path> {
        let mut paths = Vec::new();
        if let Some(ref main) = self.main_file {
            paths.push(main.as_path());
        }
        for sub in &self.subagent_files {
            paths.push(sub.as_path());
        }
        paths
    }

    /// effective mtime 对应的日期
    #[allow(dead_code)]
    fn effective_date(&self) -> NaiveDate {
        chrono::DateTime::<Local>::from(self.effective_mtime).date_naive()
    }

    /// 文件总数
    fn file_count(&self) -> usize {
        self.main_file.iter().count() + self.subagent_files.len()
    }
}

impl ArchiveAllResult {
    /// 是否有工作完成
    pub fn has_work(&self) -> bool {
        self.daily_archived > 0
            || self.weekly_merged > 0
            || self.monthly_merged > 0
            || self.yearly_merged > 0
    }

    /// 是否有错误
    pub fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }
}

impl ArchiveService {
    /// 创建归档服务
    pub fn new(source_dir: PathBuf, archive_dir: PathBuf) -> Result<Self> {
        std::fs::create_dir_all(&archive_dir)?;

        let state = ArchiveState::load(&archive_dir)?;

        Ok(Self {
            source_dir,
            archive_dir,
            compressor: Compressor::new(),
            state,
            quiet_period: 24 * 3600, // 24 小时
        })
    }

    /// 设置文件静默时间
    pub fn with_quiet_period(mut self, seconds: u64) -> Self {
        self.quiet_period = seconds;
        self
    }

    /// 尝试获取锁，防止并发执行
    pub fn try_lock(&self) -> Result<Option<fs2::FileLock>> {
        use fs2::FileExt;

        let lock_path = self.archive_dir.join(".lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&lock_path)?;

        match file.try_lock_exclusive() {
            Ok(()) => Ok(Some(fs2::FileLock::new(file))),
            Err(_) => Ok(None),
        }
    }

    /// 执行每日归档
    pub fn archive_daily(&mut self) -> Result<Option<ArchiveResult>> {
        let today = Local::now().date_naive();
        let yesterday = today - chrono::Duration::days(1);

        // 查找符合条件的 session（effective_mtime > quiet_period 且属于昨天）
        let sessions = self.find_archivable_sessions(yesterday)?;

        if sessions.is_empty() {
            tracing::info!("No sessions to archive");
            return Ok(None);
        }

        // 生成归档路径
        let archive_path = self.daily_archive_path(yesterday);
        std::fs::create_dir_all(archive_path.parent().unwrap())?;

        // 执行归档
        let result = self.do_archive(&sessions, &archive_path)?;

        // 更新状态
        self.state.record_success(&archive_path)?;

        Ok(Some(result))
    }

    /// 执行周合并
    pub fn merge_weekly(&mut self) -> Result<Option<ArchiveResult>> {
        let today = Local::now().date_naive();

        // 找到上周一
        let days_since_monday = today.weekday().num_days_from_monday();
        let last_monday = today - chrono::Duration::days(days_since_monday as i64 + 7);
        let last_sunday = last_monday + chrono::Duration::days(6);

        // 查找上周的日包
        let daily_archives = self.find_daily_archives(last_monday, last_sunday)?;

        if daily_archives.is_empty() {
            return Ok(None);
        }

        // 生成周包路径（使用日期范围）
        let archive_path = self.weekly_archive_path(last_monday, last_sunday);

        // 合并
        let result = self.do_merge(&daily_archives, &archive_path)?;

        // 删除日包
        for path in &daily_archives {
            std::fs::remove_file(path)?;
        }

        self.state.record_success(&archive_path)?;

        Ok(Some(result))
    }

    /// 执行月合并
    pub fn merge_monthly(&mut self) -> Result<Option<ArchiveResult>> {
        let today = Local::now().date_naive();
        let last_month = if today.month() == 1 {
            NaiveDate::from_ymd_opt(today.year() - 1, 12, 1).unwrap()
        } else {
            NaiveDate::from_ymd_opt(today.year(), today.month() - 1, 1).unwrap()
        };

        // 查找上月的周包
        let weekly_archives = self.find_weekly_archives(last_month.year(), last_month.month())?;

        if weekly_archives.is_empty() {
            return Ok(None);
        }

        // 生成月包路径
        let archive_path = self.monthly_archive_path(last_month.year(), last_month.month());

        // 合并
        let result = self.do_merge(&weekly_archives, &archive_path)?;

        // 删除周包
        for path in &weekly_archives {
            std::fs::remove_file(path)?;
        }

        // 清理空目录
        let week_dir = self
            .archive_dir
            .join(format!("{}", last_month.year()))
            .join(format!("{:02}", last_month.month()));
        let _ = std::fs::remove_dir(&week_dir);

        self.state.record_success(&archive_path)?;

        Ok(Some(result))
    }

    /// 执行年合并
    pub fn merge_yearly(&mut self) -> Result<Option<ArchiveResult>> {
        let today = Local::now().date_naive();
        let last_year = today.year() - 1;

        // 查找去年的月包
        let monthly_archives = self.find_monthly_archives(last_year)?;

        if monthly_archives.is_empty() {
            return Ok(None);
        }

        // 生成年包路径
        let archive_path = self.yearly_archive_path(last_year);

        // 合并
        let result = self.do_merge(&monthly_archives, &archive_path)?;

        // 删除月包
        for path in &monthly_archives {
            std::fs::remove_file(path)?;
        }

        // 清理空目录
        let year_dir = self.archive_dir.join(format!("{}", last_year));
        let _ = std::fs::remove_dir_all(&year_dir);

        self.state.record_success(&archive_path)?;

        Ok(Some(result))
    }

    /// 统一入口：检查并执行所有需要的归档（幂等）
    ///
    /// 触发时机：服务启动时 + 每日定时
    /// 幂等性：归档文件存在则跳过，已被周包覆盖的日期也跳过
    pub fn check_and_archive_all(&mut self) -> Result<ArchiveAllResult> {
        let mut result = ArchiveAllResult::default();
        let today = Local::now().date_naive();

        // 1. 检查过去 30 天内未归档的日期
        for days_ago in 1..=30 {
            let date = today - chrono::Duration::days(days_ago);
            let archive_path = self.daily_archive_path(date);

            // 幂等：已存在则跳过
            if archive_path.exists() {
                continue;
            }

            // 检查是否被周包覆盖（周包合并后会删除日包）
            if self.is_date_covered_by_weekly(date)? {
                continue;
            }

            // 检查是否有可归档的 session
            let sessions = self.find_archivable_sessions(date)?;
            if sessions.is_empty() {
                continue;
            }

            let file_count: usize = sessions.iter().map(|s| s.file_count()).sum();
            tracing::info!(
                "Compensation archive: {} ({} sessions, {} files)",
                date,
                sessions.len(),
                file_count
            );
            std::fs::create_dir_all(archive_path.parent().unwrap())?;

            match self.do_archive(&sessions, &archive_path) {
                Ok(r) => {
                    self.state.record_success(&archive_path)?;
                    result.daily_archived += 1;
                    result.files_archived += r.files_count;
                }
                Err(e) => {
                    tracing::error!("Archive failed {}: {}", date, e);
                    result.errors.push(format!("Daily archive {}: {}", date, e));
                }
            }
        }

        // 2. 检查过去 4 周内未合并的周
        for weeks_ago in 1..=4 {
            let week_start = today
                - chrono::Duration::days(today.weekday().num_days_from_monday() as i64)
                - chrono::Duration::weeks(weeks_ago);
            let week_end = week_start + chrono::Duration::days(6);

            let archive_path = self.weekly_archive_path(week_start, week_end);

            // 幂等：已存在则跳过
            if archive_path.exists() {
                continue;
            }

            let daily_archives = self.find_daily_archives(week_start, week_end)?;
            if daily_archives.is_empty() {
                continue;
            }

            tracing::info!(
                "Compensation weekly merge: {:02}-{:02}_{:02}-{:02} ({} daily archives)",
                week_start.month(),
                week_start.day(),
                week_end.month(),
                week_end.day(),
                daily_archives.len()
            );

            match self.do_merge(&daily_archives, &archive_path) {
                Ok(_) => {
                    // Delete daily archives
                    for path in &daily_archives {
                        let _ = std::fs::remove_file(path);
                    }
                    self.state.record_success(&archive_path)?;
                    result.weekly_merged += 1;
                }
                Err(e) => {
                    let range = format!(
                        "{:02}-{:02}_{:02}-{:02}",
                        week_start.month(),
                        week_start.day(),
                        week_end.month(),
                        week_end.day()
                    );
                    tracing::error!("Weekly merge failed {}: {}", range, e);
                    result.errors.push(format!("Weekly merge {}: {}", range, e));
                }
            }
        }

        // 3. 检查过去 12 月内未合并的月
        for months_ago in 1i32..=12 {
            let target_month = if today.month() as i32 - months_ago <= 0 {
                let year = today.year() - 1 - (months_ago - today.month() as i32) / 12;
                let month = ((today.month() as i32 - months_ago - 1) % 12 + 12) % 12 + 1;
                NaiveDate::from_ymd_opt(year, month as u32, 1).unwrap()
            } else {
                NaiveDate::from_ymd_opt(today.year(), today.month() - months_ago as u32, 1).unwrap()
            };

            let archive_path = self.monthly_archive_path(target_month.year(), target_month.month());

            // 幂等：已存在则跳过
            if archive_path.exists() {
                continue;
            }

            let weekly_archives =
                self.find_weekly_archives(target_month.year(), target_month.month())?;
            if weekly_archives.is_empty() {
                continue;
            }

            tracing::info!(
                "Compensation monthly merge: {}-{:02} ({} weekly archives)",
                target_month.year(),
                target_month.month(),
                weekly_archives.len()
            );

            match self.do_merge(&weekly_archives, &archive_path) {
                Ok(_) => {
                    // Delete weekly archives
                    for path in &weekly_archives {
                        let _ = std::fs::remove_file(path);
                    }
                    // Cleanup empty directory
                    let week_dir = self
                        .archive_dir
                        .join(format!("{}", target_month.year()))
                        .join(format!("{:02}", target_month.month()));
                    let _ = std::fs::remove_dir(&week_dir);

                    self.state.record_success(&archive_path)?;
                    result.monthly_merged += 1;
                }
                Err(e) => {
                    tracing::error!(
                        "Monthly merge failed {}-{:02}: {}",
                        target_month.year(),
                        target_month.month(),
                        e
                    );
                    result.errors.push(format!(
                        "Monthly merge {}-{:02}: {}",
                        target_month.year(),
                        target_month.month(),
                        e
                    ));
                }
            }
        }

        // 4. 检查上年是否合并
        let last_year = today.year() - 1;
        let yearly_path = self.yearly_archive_path(last_year);

        if !yearly_path.exists() {
            let monthly_archives = self.find_monthly_archives(last_year)?;
            if !monthly_archives.is_empty() {
                tracing::info!(
                    "Compensation yearly merge: {} ({} monthly archives)",
                    last_year,
                    monthly_archives.len()
                );

                match self.do_merge(&monthly_archives, &yearly_path) {
                    Ok(_) => {
                        // Delete monthly archives
                        for path in &monthly_archives {
                            let _ = std::fs::remove_file(path);
                        }
                        // Cleanup empty directory
                        let year_dir = self.archive_dir.join(format!("{}", last_year));
                        let _ = std::fs::remove_dir_all(&year_dir);

                        self.state.record_success(&yearly_path)?;
                        result.yearly_merged += 1;
                    }
                    Err(e) => {
                        tracing::error!("Yearly merge failed {}: {}", last_year, e);
                        result
                            .errors
                            .push(format!("Yearly merge {}: {}", last_year, e));
                    }
                }
            }
        }

        Ok(result)
    }

    /// 查找可归档的会话（以 session 为原子单位，包含 subagent）
    fn find_archivable_sessions(&self, date: NaiveDate) -> Result<Vec<ArchivableSession>> {
        let mut sessions = Vec::new();
        let now = SystemTime::now();
        let quiet_duration = Duration::from_secs(self.quiet_period);

        if !self.source_dir.exists() {
            return Ok(sessions);
        }

        // 遍历所有项目目录
        for entry in std::fs::read_dir(&self.source_dir)? {
            let entry = entry?;
            let project_dir = entry.path();

            if !project_dir.is_dir() {
                continue;
            }

            // 收集该项目下的所有 session
            // key: uuid, value: (main_file, subagent_files, max_mtime)
            let mut session_map: HashMap<String, (Option<PathBuf>, Vec<PathBuf>, SystemTime)> =
                HashMap::new();

            // 第一遍：扫描 .jsonl 主文件
            for file_entry in std::fs::read_dir(&project_dir)? {
                let file_entry = file_entry?;
                let file_path = file_entry.path();

                if !file_path.is_file() {
                    continue;
                }

                if file_path.extension().is_none_or(|e| e != "jsonl") {
                    continue;
                }

                let uuid = match file_path.file_stem().and_then(|s| s.to_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };

                let mtime = std::fs::metadata(&file_path)?.modified()?;
                let rel_path = match file_path.strip_prefix(&self.source_dir) {
                    Ok(p) => p.to_path_buf(),
                    Err(_) => {
                        tracing::warn!("Skipping file outside source_dir: {:?}", file_path);
                        continue;
                    }
                };

                // 路径安全检查
                if rel_path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    tracing::warn!("Skipping path with ..: {:?}", rel_path);
                    continue;
                }

                let entry = session_map
                    .entry(uuid)
                    .or_insert_with(|| (None, Vec::new(), SystemTime::UNIX_EPOCH));
                entry.0 = Some(rel_path);
                if mtime > entry.2 {
                    entry.2 = mtime;
                }
            }

            // 第二遍：扫描 subagent 目录
            for file_entry in std::fs::read_dir(&project_dir)? {
                let file_entry = file_entry?;
                let dir_path = file_entry.path();

                if !dir_path.is_dir() {
                    continue;
                }

                let uuid = match dir_path.file_name().and_then(|s| s.to_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };

                let subagents_dir = dir_path.join("subagents");
                if !subagents_dir.is_dir() {
                    continue;
                }

                let subagent_entries = match std::fs::read_dir(&subagents_dir) {
                    Ok(entries) => entries,
                    Err(_) => continue,
                };

                for sub_entry in subagent_entries {
                    let sub_entry = match sub_entry {
                        Ok(e) => e,
                        Err(_) => continue,
                    };
                    let sub_path = sub_entry.path();

                    if !sub_path.is_file() {
                        continue;
                    }
                    if sub_path.extension().is_none_or(|e| e != "jsonl") {
                        continue;
                    }

                    let mtime = match std::fs::metadata(&sub_path) {
                        Ok(m) => match m.modified() {
                            Ok(t) => t,
                            Err(_) => continue,
                        },
                        Err(_) => continue,
                    };

                    let rel_path = match sub_path.strip_prefix(&self.source_dir) {
                        Ok(p) => p.to_path_buf(),
                        Err(_) => {
                            tracing::warn!("Skipping file outside source_dir: {:?}", sub_path);
                            continue;
                        }
                    };

                    // 路径安全检查
                    if rel_path
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
                    {
                        tracing::warn!("Skipping path with ..: {:?}", rel_path);
                        continue;
                    }

                    let entry = session_map
                        .entry(uuid.clone())
                        .or_insert_with(|| (None, Vec::new(), SystemTime::UNIX_EPOCH));
                    entry.1.push(rel_path);
                    if mtime > entry.2 {
                        entry.2 = mtime;
                    }
                }
            }

            // 构建 ArchivableSession 并过滤
            for (uuid, (main_file, subagent_files, effective_mtime)) in session_map {
                // 跳过空 session（无主文件且无 subagent）
                if main_file.is_none() && subagent_files.is_empty() {
                    continue;
                }

                // quiet_period 检查（用 effective_mtime）
                if now.duration_since(effective_mtime).unwrap_or_default() < quiet_duration {
                    continue;
                }

                // 日期匹配（用 effective_mtime）
                let mtime_date = chrono::DateTime::<Local>::from(effective_mtime).date_naive();
                if mtime_date != date {
                    continue;
                }

                sessions.push(ArchivableSession {
                    uuid,
                    main_file,
                    subagent_files,
                    effective_mtime,
                });
            }
        }

        Ok(sessions)
    }

    /// 执行归档（session-based，保留目录结构）
    fn do_archive(
        &self,
        sessions: &[ArchivableSession],
        archive_path: &Path,
    ) -> Result<ArchiveResult> {
        let tmp_path = archive_path.with_extension("tar.xz.tmp");

        // 提取所有相对路径，计算 SHA-256
        let mut original_hashes = Vec::new();
        let mut original_size = 0u64;
        let mut all_relative_paths = Vec::new();

        for session in sessions {
            for rel_path in session.all_relative_paths() {
                let abs_path = self.source_dir.join(rel_path);
                let hash = self.compressor.sha256_file(&abs_path)?;
                let size = std::fs::metadata(&abs_path)?.len();
                original_hashes.push((rel_path.to_path_buf(), hash, size));
                original_size += size;
                all_relative_paths.push(rel_path.to_path_buf());
            }
        }

        // 压缩（保留目录结构）
        self.compressor.compress_with_structure(
            &self.source_dir,
            &all_relative_paths,
            &tmp_path,
        )?;

        // 解压验证
        let verify_dir = self.archive_dir.join(".verify");
        std::fs::create_dir_all(&verify_dir)?;

        self.compressor.decompress(&tmp_path, &verify_dir)?;

        // 校验 SHA-256（使用相对路径查找解压文件）
        for (rel_path, original_hash, _) in &original_hashes {
            let extracted_path = verify_dir.join(rel_path);

            if !extracted_path.exists() {
                std::fs::remove_dir_all(&verify_dir)?;
                std::fs::remove_file(&tmp_path)?;
                anyhow::bail!("验证失败：文件缺失 {:?}", rel_path);
            }

            let extracted_hash = self.compressor.sha256_file(&extracted_path)?;
            if extracted_hash != *original_hash {
                std::fs::remove_dir_all(&verify_dir)?;
                std::fs::remove_file(&tmp_path)?;
                anyhow::bail!("验证失败：SHA-256 不匹配 {:?}", rel_path);
            }
        }

        // 清理验证目录
        std::fs::remove_dir_all(&verify_dir)?;

        // 原子 rename
        std::fs::rename(&tmp_path, archive_path)?;

        let compressed_size = std::fs::metadata(archive_path)?.len();
        let files_count: usize = sessions.iter().map(|s| s.file_count()).sum();

        Ok(ArchiveResult {
            archive_path: archive_path.to_path_buf(),
            files_count,
            original_size,
            compressed_size,
            compression_ratio: original_size as f64 / compressed_size as f64,
        })
    }

    /// 执行合并（支持递归目录结构 + 去重）
    fn do_merge(&self, archives: &[PathBuf], target_path: &Path) -> Result<ArchiveResult> {
        use std::collections::HashSet;

        // 确保目标目录存在
        std::fs::create_dir_all(target_path.parent().unwrap())?;

        let tmp_path = target_path.with_extension("tar.xz.tmp");
        let merge_dir = self.archive_dir.join(".merge");
        if merge_dir.exists() {
            std::fs::remove_dir_all(&merge_dir)?;
        }
        std::fs::create_dir_all(&merge_dir)?;

        // 归档列表排序（保证确定性，按路径名排序 = 按时间升序）
        let mut sorted_archives: Vec<&PathBuf> = archives.iter().collect();
        sorted_archives.sort();

        // 解压每个归档到隔离子目录（防止覆盖）
        let mut sub_dirs = Vec::new();
        for (i, archive) in sorted_archives.iter().enumerate() {
            let sub_dir = merge_dir.join(format!("{}", i));
            std::fs::create_dir_all(&sub_dir)?;
            self.compressor.decompress(archive, &sub_dir)?;
            sub_dirs.push(sub_dir);
        }

        // 递归收集所有文件，按相对路径去重（先遇到的 wins）
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut unique_relative_paths: Vec<PathBuf> = Vec::new();
        // 记录第一个源，用于后续复制
        let mut source_map: HashMap<PathBuf, PathBuf> = HashMap::new();

        for sub_dir in &sub_dirs {
            for entry in walkdir::WalkDir::new(sub_dir) {
                let entry = entry?;
                if !entry.file_type().is_file() {
                    continue;
                }
                let rel_path = entry.path().strip_prefix(sub_dir)?.to_path_buf();

                if seen.contains(&rel_path) {
                    // 相同路径已存在，检查是否内容不同
                    let existing_source = &source_map[&rel_path];
                    let existing_hash = self.compressor.sha256_file(existing_source)?;
                    let new_hash = self.compressor.sha256_file(entry.path())?;
                    if existing_hash != new_hash {
                        tracing::warn!(
                            "Merge conflict: {:?} has different content across archives, keeping earlier version",
                            rel_path
                        );
                    }
                    continue;
                }

                seen.insert(rel_path.clone());
                unique_relative_paths.push(rel_path.clone());
                source_map.insert(rel_path, entry.path().to_path_buf());
            }
        }

        // 复制去重后的文件到统一目录，保持目录结构
        let unified_dir = merge_dir.join("unified");
        std::fs::create_dir_all(&unified_dir)?;

        let mut original_size = 0u64;
        for rel_path in &unique_relative_paths {
            let src = &source_map[rel_path];
            let dst = unified_dir.join(rel_path);
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(src, &dst)?;
            original_size += std::fs::metadata(&dst)?.len();
        }

        // 用 compress_with_structure 重新打包
        self.compressor
            .compress_with_structure(&unified_dir, &unique_relative_paths, &tmp_path)?;

        // 清理合并目录
        std::fs::remove_dir_all(&merge_dir)?;

        // 原子 rename
        std::fs::rename(&tmp_path, target_path)?;

        let compressed_size = std::fs::metadata(target_path)?.len();

        Ok(ArchiveResult {
            archive_path: target_path.to_path_buf(),
            files_count: unique_relative_paths.len(),
            original_size,
            compressed_size,
            compression_ratio: original_size as f64 / compressed_size as f64,
        })
    }

    // === 路径生成 ===

    fn daily_archive_path(&self, date: NaiveDate) -> PathBuf {
        self.archive_dir
            .join(format!("{}", date.year()))
            .join(format!("{:02}", date.month()))
            .join(format!("W{}", date.iso_week().week()))
            .join(format!("{:02}.tar.xz", date.day()))
    }

    /// 周包路径：使用日期范围命名 (12-02_12-08.tar.xz)
    fn weekly_archive_path(&self, week_start: NaiveDate, week_end: NaiveDate) -> PathBuf {
        self.archive_dir
            .join(format!("{}", week_start.year()))
            .join(format!("{:02}", week_start.month()))
            .join(format!(
                "{:02}-{:02}_{:02}-{:02}.tar.xz",
                week_start.month(),
                week_start.day(),
                week_end.month(),
                week_end.day()
            ))
    }

    fn monthly_archive_path(&self, year: i32, month: u32) -> PathBuf {
        self.archive_dir
            .join(format!("{}", year))
            .join(format!("{:02}.tar.xz", month))
    }

    fn yearly_archive_path(&self, year: i32) -> PathBuf {
        self.archive_dir.join(format!("{}.tar.xz", year))
    }

    // === 查找归档 ===

    fn find_daily_archives(&self, from: NaiveDate, to: NaiveDate) -> Result<Vec<PathBuf>> {
        let mut archives = Vec::new();
        let mut date = from;

        while date <= to {
            let path = self.daily_archive_path(date);
            if path.exists() {
                archives.push(path);
            }
            date += chrono::Duration::days(1);
        }

        Ok(archives)
    }

    /// 查找周包（支持新格式 12-02_12-08.tar.xz 和旧格式 W49.tar.xz）
    fn find_weekly_archives(&self, year: i32, month: u32) -> Result<Vec<PathBuf>> {
        let mut archives = Vec::new();
        let month_dir = self
            .archive_dir
            .join(format!("{}", year))
            .join(format!("{:02}", month));

        if !month_dir.exists() {
            return Ok(archives);
        }

        // 匹配日期范围格式：12-02_12-08.tar.xz
        let date_range_pattern = regex::Regex::new(r"^\d{2}-\d{2}_\d{2}-\d{2}\.tar$").unwrap();

        for entry in std::fs::read_dir(&month_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() && path.extension().is_some_and(|e| e == "xz") {
                let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                // 支持新格式（日期范围）和旧格式（W周号）
                if date_range_pattern.is_match(name)
                    || (name.starts_with("W") && name.ends_with(".tar"))
                {
                    archives.push(path);
                }
            }
        }

        Ok(archives)
    }

    fn find_monthly_archives(&self, year: i32) -> Result<Vec<PathBuf>> {
        let mut archives = Vec::new();
        let year_dir = self.archive_dir.join(format!("{}", year));

        if !year_dir.exists() {
            return Ok(archives);
        }

        for entry in std::fs::read_dir(&year_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() && path.extension().is_some_and(|e| e == "xz") {
                archives.push(path);
            }
        }

        Ok(archives)
    }

    /// 检查日期是否被周包覆盖
    ///
    /// 周包合并后日包会被删除，需要通过周包的日期范围判断
    fn is_date_covered_by_weekly(&self, date: NaiveDate) -> Result<bool> {
        // 计算该日期所在周的起止日期
        let days_since_monday = date.weekday().num_days_from_monday() as i64;
        let week_start = date - chrono::Duration::days(days_since_monday);
        let week_end = week_start + chrono::Duration::days(6);

        // 检查周包是否存在
        let weekly_path = self.weekly_archive_path(week_start, week_end);
        if weekly_path.exists() {
            return Ok(true);
        }

        // 检查月包是否存在（月包合并后周包被删除）
        let monthly_path = self.monthly_archive_path(date.year(), date.month());
        if monthly_path.exists() {
            return Ok(true);
        }

        // 检查年包是否存在
        let yearly_path = self.yearly_archive_path(date.year());
        if yearly_path.exists() {
            return Ok(true);
        }

        Ok(false)
    }

    /// 获取归档状态
    pub fn status(&self) -> &ArchiveState {
        &self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    /// 创建模拟 JSONL 文件并设置指定 mtime
    fn create_jsonl(path: &Path, content: &str, mtime: SystemTime) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        drop(f);
        filetime::set_file_mtime(path, filetime::FileTime::from_system_time(mtime)).unwrap();
    }

    /// 两天前的时间（满足 quiet_period）
    fn two_days_ago() -> SystemTime {
        SystemTime::now() - Duration::from_secs(2 * 24 * 3600)
    }

    /// 两天前的日期
    fn two_days_ago_date() -> NaiveDate {
        let dt = chrono::DateTime::<Local>::from(two_days_ago());
        dt.date_naive()
    }

    #[test]
    fn test_find_archivable_sessions_main_only() {
        let source_dir = TempDir::new().unwrap();
        let archive_dir = TempDir::new().unwrap();

        let mtime = two_days_ago();
        let date = two_days_ago_date();

        // 创建 project/uuid.jsonl（无 subagent）
        let project_dir = source_dir.path().join("-Users-test-project");
        create_jsonl(
            &project_dir.join("abc123.jsonl"),
            r#"{"type":"user","message":"hello"}"#,
            mtime,
        );

        let svc = ArchiveService::new(
            source_dir.path().to_path_buf(),
            archive_dir.path().to_path_buf(),
        )
        .unwrap();

        let sessions = svc.find_archivable_sessions(date).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].uuid, "abc123");
        assert!(sessions[0].main_file.is_some());
        assert!(sessions[0].subagent_files.is_empty());
        assert_eq!(sessions[0].file_count(), 1);
    }

    #[test]
    fn test_find_archivable_sessions_with_subagents() {
        let source_dir = TempDir::new().unwrap();
        let archive_dir = TempDir::new().unwrap();

        let mtime = two_days_ago();
        let date = two_days_ago_date();

        let project_dir = source_dir.path().join("-Users-test-project");

        // 主文件
        create_jsonl(
            &project_dir.join("abc123.jsonl"),
            r#"{"type":"user","message":"hello"}"#,
            mtime,
        );
        // subagent 文件
        create_jsonl(
            &project_dir.join("abc123/subagents/agent-001.jsonl"),
            r#"{"type":"user","message":"sub1"}"#,
            mtime,
        );
        create_jsonl(
            &project_dir.join("abc123/subagents/agent-002.jsonl"),
            r#"{"type":"user","message":"sub2"}"#,
            mtime,
        );

        let svc = ArchiveService::new(
            source_dir.path().to_path_buf(),
            archive_dir.path().to_path_buf(),
        )
        .unwrap();

        let sessions = svc.find_archivable_sessions(date).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].uuid, "abc123");
        assert!(sessions[0].main_file.is_some());
        assert_eq!(sessions[0].subagent_files.len(), 2);
        assert_eq!(sessions[0].file_count(), 3);
    }

    #[test]
    fn test_find_archivable_sessions_orphan_subagent() {
        let source_dir = TempDir::new().unwrap();
        let archive_dir = TempDir::new().unwrap();

        let mtime = two_days_ago();
        let date = two_days_ago_date();

        let project_dir = source_dir.path().join("-Users-test-project");

        // 只有 subagent 目录，无主文件
        create_jsonl(
            &project_dir.join("orphan-uuid/subagents/agent-001.jsonl"),
            r#"{"type":"user","message":"orphan"}"#,
            mtime,
        );

        let svc = ArchiveService::new(
            source_dir.path().to_path_buf(),
            archive_dir.path().to_path_buf(),
        )
        .unwrap();

        let sessions = svc.find_archivable_sessions(date).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].uuid, "orphan-uuid");
        assert!(sessions[0].main_file.is_none());
        assert_eq!(sessions[0].subagent_files.len(), 1);
    }

    #[test]
    fn test_find_archivable_sessions_effective_mtime() {
        let source_dir = TempDir::new().unwrap();
        let archive_dir = TempDir::new().unwrap();

        // 主文件 3 天前，subagent 2 天前
        // effective_mtime 应该取 subagent 的 mtime（2 天前）
        let three_days_ago = SystemTime::now() - Duration::from_secs(3 * 24 * 3600);
        let mtime_sub = two_days_ago();
        let date = two_days_ago_date();

        let project_dir = source_dir.path().join("-Users-test-project");
        create_jsonl(
            &project_dir.join("sess1.jsonl"),
            r#"{"type":"user"}"#,
            three_days_ago,
        );
        create_jsonl(
            &project_dir.join("sess1/subagents/agent-001.jsonl"),
            r#"{"type":"user"}"#,
            mtime_sub,
        );

        let svc = ArchiveService::new(
            source_dir.path().to_path_buf(),
            archive_dir.path().to_path_buf(),
        )
        .unwrap();

        // 搜索 2 天前的日期，应该找到（因为 effective mtime = subagent mtime = 2 天前）
        let sessions = svc.find_archivable_sessions(date).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].uuid, "sess1");
        assert_eq!(sessions[0].effective_date(), date);
    }

    #[test]
    fn test_find_archivable_sessions_quiet_period_not_met() {
        let source_dir = TempDir::new().unwrap();
        let archive_dir = TempDir::new().unwrap();

        // 文件刚创建（mtime = now），不满足 quiet_period
        let now = SystemTime::now();
        let today = Local::now().date_naive();

        let project_dir = source_dir.path().join("-Users-test-project");
        create_jsonl(&project_dir.join("recent.jsonl"), r#"{"type":"user"}"#, now);

        let svc = ArchiveService::new(
            source_dir.path().to_path_buf(),
            archive_dir.path().to_path_buf(),
        )
        .unwrap();

        let sessions = svc.find_archivable_sessions(today).unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn test_find_archivable_sessions_skips_non_jsonl() {
        let source_dir = TempDir::new().unwrap();
        let archive_dir = TempDir::new().unwrap();

        let mtime = two_days_ago();
        let date = two_days_ago_date();

        let project_dir = source_dir.path().join("-Users-test-project");
        // .jsonl 文件应该被收集
        create_jsonl(&project_dir.join("abc.jsonl"), r#"{"type":"user"}"#, mtime);
        // .txt 文件应该被跳过
        create_jsonl(
            &project_dir.join("abc/subagents/notes.txt"),
            "some notes",
            mtime,
        );

        let svc = ArchiveService::new(
            source_dir.path().to_path_buf(),
            archive_dir.path().to_path_buf(),
        )
        .unwrap();

        let sessions = svc.find_archivable_sessions(date).unwrap();
        assert_eq!(sessions.len(), 1);
        // 只有主文件，subagent 中的 .txt 被忽略
        assert_eq!(sessions[0].subagent_files.len(), 0);
    }

    #[test]
    fn test_compress_with_structure_preserves_paths() {
        let base_dir = TempDir::new().unwrap();
        let output_dir = TempDir::new().unwrap();

        // 创建模拟的项目结构（以 - 开头的路径）
        let project = "-Users-test-project";
        let main_rel = PathBuf::from(format!("{}/abc123.jsonl", project));
        let sub_rel = PathBuf::from(format!("{}/abc123/subagents/agent-001.jsonl", project));

        let main_path = base_dir.path().join(&main_rel);
        let sub_path = base_dir.path().join(&sub_rel);

        std::fs::create_dir_all(main_path.parent().unwrap()).unwrap();
        std::fs::create_dir_all(sub_path.parent().unwrap()).unwrap();
        std::fs::write(&main_path, r#"{"type":"user","message":"main"}"#).unwrap();
        std::fs::write(&sub_path, r#"{"type":"user","message":"sub"}"#).unwrap();

        let archive_path = output_dir.path().join("test.tar.xz");
        let compressor = Compressor::new().with_nice(false);

        compressor
            .compress_with_structure(
                base_dir.path(),
                &[main_rel.clone(), sub_rel.clone()],
                &archive_path,
            )
            .unwrap();

        assert!(archive_path.exists());

        // 解压验证目录结构
        let extract_dir = output_dir.path().join("extracted");
        compressor.decompress(&archive_path, &extract_dir).unwrap();

        // 验证解压后的文件存在于正确的相对路径
        assert!(extract_dir.join(&main_rel).exists());
        assert!(extract_dir.join(&sub_rel).exists());

        // 验证内容
        let main_content = std::fs::read_to_string(extract_dir.join(&main_rel)).unwrap();
        assert!(main_content.contains("main"));
        let sub_content = std::fs::read_to_string(extract_dir.join(&sub_rel)).unwrap();
        assert!(sub_content.contains("sub"));
    }

    #[test]
    fn test_do_archive_and_verify() {
        let source_dir = TempDir::new().unwrap();
        let archive_dir = TempDir::new().unwrap();

        let mtime = two_days_ago();

        // 创建项目结构
        let project = "-Users-test-project";
        let project_dir = source_dir.path().join(project);

        create_jsonl(
            &project_dir.join("sess1.jsonl"),
            r#"{"type":"user","message":"hello"}"#,
            mtime,
        );
        create_jsonl(
            &project_dir.join("sess1/subagents/agent-001.jsonl"),
            r#"{"type":"user","message":"sub"}"#,
            mtime,
        );

        let svc = ArchiveService::new(
            source_dir.path().to_path_buf(),
            archive_dir.path().to_path_buf(),
        )
        .unwrap();

        let sessions = vec![ArchivableSession {
            uuid: "sess1".to_string(),
            main_file: Some(PathBuf::from(format!("{}/sess1.jsonl", project))),
            subagent_files: vec![PathBuf::from(format!(
                "{}/sess1/subagents/agent-001.jsonl",
                project
            ))],
            effective_mtime: mtime,
        }];

        let archive_path = archive_dir.path().join("test.tar.xz");
        let result = svc.do_archive(&sessions, &archive_path).unwrap();

        assert_eq!(result.files_count, 2);
        assert!(result.compressed_size > 0);
        assert!(archive_path.exists());

        // 解压验证
        let verify_dir = archive_dir.path().join("final_verify");
        svc.compressor
            .decompress(&archive_path, &verify_dir)
            .unwrap();

        assert!(verify_dir.join(format!("{}/sess1.jsonl", project)).exists());
        assert!(verify_dir
            .join(format!("{}/sess1/subagents/agent-001.jsonl", project))
            .exists());
    }

    #[test]
    fn test_do_merge_dedup_and_structure() {
        let source_dir = TempDir::new().unwrap();
        let archive_dir = TempDir::new().unwrap();

        let compressor = Compressor::new().with_nice(false);

        let project = "-Users-test-project";

        // 创建第一个归档（包含 file-a 和 file-b）
        let base1 = TempDir::new().unwrap();
        let a1 = base1.path().join(format!("{}/a.jsonl", project));
        let b1 = base1.path().join(format!("{}/b.jsonl", project));
        std::fs::create_dir_all(a1.parent().unwrap()).unwrap();
        std::fs::write(&a1, "content-a-1").unwrap();
        std::fs::write(&b1, "content-b").unwrap();

        let archive1 = archive_dir.path().join("01.tar.xz");
        compressor
            .compress_with_structure(
                base1.path(),
                &[
                    PathBuf::from(format!("{}/a.jsonl", project)),
                    PathBuf::from(format!("{}/b.jsonl", project)),
                ],
                &archive1,
            )
            .unwrap();

        // 创建第二个归档（包含 file-a 不同内容 和 file-c）
        let base2 = TempDir::new().unwrap();
        let a2 = base2.path().join(format!("{}/a.jsonl", project));
        let c2 = base2.path().join(format!("{}/c.jsonl", project));
        std::fs::create_dir_all(a2.parent().unwrap()).unwrap();
        std::fs::write(&a2, "content-a-2-different").unwrap();
        std::fs::write(&c2, "content-c").unwrap();

        let archive2 = archive_dir.path().join("02.tar.xz");
        compressor
            .compress_with_structure(
                base2.path(),
                &[
                    PathBuf::from(format!("{}/a.jsonl", project)),
                    PathBuf::from(format!("{}/c.jsonl", project)),
                ],
                &archive2,
            )
            .unwrap();

        // Merge
        let svc = ArchiveService::new(
            source_dir.path().to_path_buf(),
            archive_dir.path().to_path_buf(),
        )
        .unwrap();

        let merged_path = archive_dir.path().join("merged.tar.xz");
        let result = svc.do_merge(&[archive1, archive2], &merged_path).unwrap();

        // 应该有 3 个文件（a、b、c，a 去重只保留一份）
        assert_eq!(result.files_count, 3);

        // 解压验证
        let verify_dir = archive_dir.path().join("verify_merge");
        compressor.decompress(&merged_path, &verify_dir).unwrap();

        assert!(verify_dir.join(format!("{}/a.jsonl", project)).exists());
        assert!(verify_dir.join(format!("{}/b.jsonl", project)).exists());
        assert!(verify_dir.join(format!("{}/c.jsonl", project)).exists());

        // a.jsonl 应该保留第一个归档的版本（先遇到的 wins）
        let a_content =
            std::fs::read_to_string(verify_dir.join(format!("{}/a.jsonl", project))).unwrap();
        assert_eq!(a_content, "content-a-1");
    }
}

/// 文件锁包装
pub mod fs2 {
    use std::fs::File;

    pub struct FileLock {
        _file: File,
    }

    impl FileLock {
        pub fn new(file: File) -> Self {
            Self { _file: file }
        }
    }

    pub trait FileExt {
        fn try_lock_exclusive(&self) -> std::io::Result<()>;
    }

    impl FileExt for File {
        #[cfg(unix)]
        fn try_lock_exclusive(&self) -> std::io::Result<()> {
            use std::os::unix::io::AsRawFd;
            let fd = self.as_raw_fd();
            let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
            if ret == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        }

        #[cfg(not(unix))]
        fn try_lock_exclusive(&self) -> std::io::Result<()> {
            Ok(()) // Windows 暂不支持
        }
    }
}
