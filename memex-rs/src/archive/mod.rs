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

        // 查找符合条件的文件（mtime > quiet_period 且属于昨天）
        let files = self.find_archivable_files(yesterday)?;

        if files.is_empty() {
            tracing::info!("No files to archive");
            return Ok(None);
        }

        // 生成归档路径
        let archive_path = self.daily_archive_path(yesterday);
        std::fs::create_dir_all(archive_path.parent().unwrap())?;

        // 执行归档
        let result = self.do_archive(&files, &archive_path)?;

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

            // 检查是否有可归档的文件
            let files = self.find_archivable_files(date)?;
            if files.is_empty() {
                continue;
            }

            tracing::info!("Compensation archive: {} ({} files)", date, files.len());
            std::fs::create_dir_all(archive_path.parent().unwrap())?;

            match self.do_archive(&files, &archive_path) {
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

    /// 查找可归档的文件
    fn find_archivable_files(&self, date: NaiveDate) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        let now = SystemTime::now();
        let quiet_duration = Duration::from_secs(self.quiet_period);

        // 遍历所有项目目录
        if !self.source_dir.exists() {
            return Ok(files);
        }

        for entry in std::fs::read_dir(&self.source_dir)? {
            let entry = entry?;
            let project_dir = entry.path();

            if !project_dir.is_dir() {
                continue;
            }

            // 遍历项目内的 JSONL 文件
            for file_entry in std::fs::read_dir(&project_dir)? {
                let file_entry = file_entry?;
                let file_path = file_entry.path();

                if !file_path.is_file() {
                    continue;
                }

                if file_path.extension().is_none_or(|e| e != "jsonl") {
                    continue;
                }

                // 检查 mtime
                let metadata = std::fs::metadata(&file_path)?;
                let mtime = metadata.modified()?;

                // 必须静默超过 quiet_period
                if now.duration_since(mtime).unwrap_or_default() < quiet_duration {
                    continue;
                }

                // 检查是否属于目标日期（按 mtime 判断）
                let mtime_date = chrono::DateTime::<Local>::from(mtime).date_naive();
                if mtime_date == date {
                    files.push(file_path);
                }
            }
        }

        Ok(files)
    }

    /// 执行归档
    fn do_archive(&self, files: &[PathBuf], archive_path: &Path) -> Result<ArchiveResult> {
        let tmp_path = archive_path.with_extension("tar.xz.tmp");

        // 计算原始文件的 SHA-256
        let mut original_hashes = Vec::new();
        let mut original_size = 0u64;

        for file in files {
            let hash = self.compressor.sha256_file(file)?;
            let size = std::fs::metadata(file)?.len();
            original_hashes.push((file.clone(), hash, size));
            original_size += size;
        }

        // 压缩
        self.compressor.compress_files(files, &tmp_path)?;

        // 解压验证
        let verify_dir = self.archive_dir.join(".verify");
        std::fs::create_dir_all(&verify_dir)?;

        self.compressor.decompress(&tmp_path, &verify_dir)?;

        // 校验 SHA-256
        for (original_path, original_hash, _) in &original_hashes {
            let file_name = original_path.file_name().unwrap();
            let extracted_path = verify_dir.join(file_name);

            if !extracted_path.exists() {
                std::fs::remove_dir_all(&verify_dir)?;
                std::fs::remove_file(&tmp_path)?;
                anyhow::bail!("验证失败：文件缺失 {:?}", file_name);
            }

            let extracted_hash = self.compressor.sha256_file(&extracted_path)?;
            if extracted_hash != *original_hash {
                std::fs::remove_dir_all(&verify_dir)?;
                std::fs::remove_file(&tmp_path)?;
                anyhow::bail!("验证失败：SHA-256 不匹配 {:?}", file_name);
            }
        }

        // 清理验证目录
        std::fs::remove_dir_all(&verify_dir)?;

        // 原子 rename
        std::fs::rename(&tmp_path, archive_path)?;

        let compressed_size = std::fs::metadata(archive_path)?.len();

        Ok(ArchiveResult {
            archive_path: archive_path.to_path_buf(),
            files_count: files.len(),
            original_size,
            compressed_size,
            compression_ratio: original_size as f64 / compressed_size as f64,
        })
    }

    /// 执行合并
    fn do_merge(&self, archives: &[PathBuf], target_path: &Path) -> Result<ArchiveResult> {
        // 确保目标目录存在（compress_files 需要写入 .tar 临时文件）
        std::fs::create_dir_all(target_path.parent().unwrap())?;

        let tmp_path = target_path.with_extension("tar.xz.tmp");
        let merge_dir = self.archive_dir.join(".merge");
        std::fs::create_dir_all(&merge_dir)?;

        // 解压所有归档到合并目录
        let mut all_files = Vec::new();
        for archive in archives {
            self.compressor.decompress(archive, &merge_dir)?;
        }

        // 收集所有文件
        for entry in std::fs::read_dir(&merge_dir)? {
            let entry = entry?;
            if entry.path().is_file() {
                all_files.push(entry.path());
            }
        }

        // 重新压缩
        let original_size: u64 = all_files
            .iter()
            .map(|f| std::fs::metadata(f).map(|m| m.len()).unwrap_or(0))
            .sum();

        self.compressor.compress_files(&all_files, &tmp_path)?;

        // 清理合并目录
        std::fs::remove_dir_all(&merge_dir)?;

        // 原子 rename
        std::fs::rename(&tmp_path, target_path)?;

        let compressed_size = std::fs::metadata(target_path)?.len();

        Ok(ArchiveResult {
            archive_path: target_path.to_path_buf(),
            files_count: all_files.len(),
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
