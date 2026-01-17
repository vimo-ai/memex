//! memex - AI CLI 历史搜索工具
//!
//! 直接读取 Claude Code、Codex、OpenCode 的原始会话文件

mod output;
mod search;
mod source;
mod utils;

use anyhow::Result;
use clap::{Parser, Subcommand};
use output::OutputFormat;
use search::{SearchOptions, Searcher};
use source::{parse_source, DataSourceManager};

#[derive(Parser)]
#[command(name = "memex")]
#[command(about = "AI CLI history search - Claude Code, Codex, OpenCode")]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Search across all AI CLI histories
    Search {
        /// Search query (regex supported)
        query: String,

        /// Filter by CLI type (claude, codex, opencode, gemini)
        #[arg(short, long)]
        cli: Option<Vec<String>>,

        /// Filter by project name/path
        #[arg(short, long)]
        project: Option<String>,

        /// Only search sessions from the last N days
        #[arg(short, long)]
        days: Option<u64>,

        /// Start date filter (YYYY-MM-DD)
        #[arg(long)]
        since: Option<String>,

        /// End date filter (YYYY-MM-DD)
        #[arg(long)]
        until: Option<String>,

        /// Maximum results
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,

        /// Case insensitive search
        #[arg(short, long)]
        ignore_case: bool,

        /// Show context lines
        #[arg(short = 'C', long, default_value = "2")]
        context: usize,

        /// Output format (text, json)
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// List all sessions
    List {
        /// Filter by CLI type
        #[arg(short, long)]
        cli: Option<Vec<String>>,

        /// Filter by project
        #[arg(short, long)]
        project: Option<String>,

        /// Show recent N sessions
        #[arg(short = 'n', long, default_value = "20")]
        limit: usize,

        /// Output format (text, json)
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// View a specific session
    View {
        /// Session ID (prefix match supported)
        session: String,

        /// Show only user messages
        #[arg(long)]
        user_only: bool,

        /// Show only assistant messages
        #[arg(long)]
        assistant_only: bool,

        /// Output format (text, json, markdown)
        #[arg(short, long, default_value = "text")]
        format: String,
    },

    /// Show available data sources
    Sources {
        /// Output format (text, json)
        #[arg(short, long, default_value = "text")]
        format: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Search {
            query,
            cli,
            project,
            days,
            since,
            until,
            limit,
            ignore_case,
            context,
            format,
        } => cmd_search(query, cli, project, days, since, until, limit, ignore_case, context, format),

        Commands::List {
            cli,
            project,
            limit,
            format,
        } => cmd_list(cli, project, limit, format),

        Commands::View {
            session,
            user_only,
            assistant_only,
            format,
        } => cmd_view(session, user_only, assistant_only, format),

        Commands::Sources { format } => cmd_sources(format),
    }
}

/// 搜索命令
#[allow(clippy::too_many_arguments)]
fn cmd_search(
    query: String,
    cli_filter: Option<Vec<String>>,
    project: Option<String>,
    days: Option<u64>,
    since: Option<String>,
    until: Option<String>,
    limit: usize,
    ignore_case: bool,
    context: usize,
    format: String,
) -> Result<()> {
    let searcher = Searcher::new();

    // 解析源过滤
    let source_filter = cli_filter.map(|filters| {
        filters
            .iter()
            .filter_map(|s| parse_source(s))
            .collect()
    });

    // 计算时间范围
    let (start_time, end_time) = parse_time_range(days, since, until);

    let options = SearchOptions {
        query,
        case_insensitive: ignore_case,
        max_results: limit,
        source_filter,
        project_filter: project,
        context_lines: context,
        start_time,
        end_time,
    };

    let results = searcher.search(&options)?;

    // format 校验
    let format: OutputFormat = match format.parse() {
        Ok(f) => f,
        Err(_) => {
            eprintln!("Error: invalid format '{}'. Valid options: text, json", format);
            std::process::exit(1);
        }
    };
    let output = output::format_search_results(&results, format);
    println!("{}", output);

    Ok(())
}

/// 列表命令
fn cmd_list(
    cli_filter: Option<Vec<String>>,
    project: Option<String>,
    limit: usize,
    format: String,
) -> Result<()> {
    let manager = DataSourceManager::new();

    let mut sessions = manager.list_all_sessions()?;

    // 源过滤
    if let Some(filters) = cli_filter {
        let sources: Vec<_> = filters.iter().filter_map(|s| parse_source(s)).collect();
        if !sources.is_empty() {
            sessions.retain(|s| sources.contains(&s.source));
        }
    }

    // 项目过滤
    if let Some(ref project) = project {
        sessions.retain(|s| {
            s.project_path.contains(project)
                || s.project_name
                    .as_ref()
                    .map(|n| n.contains(project))
                    .unwrap_or(false)
        });
    }

    // 限制数量
    sessions.truncate(limit);

    // format 校验
    let format: OutputFormat = match format.parse() {
        Ok(f) => f,
        Err(_) => {
            eprintln!("Error: invalid format '{}'. Valid options: text, json", format);
            std::process::exit(1);
        }
    };
    let output = output::format_sessions(&sessions, format);
    println!("{}", output);

    Ok(())
}

/// 查看命令
fn cmd_view(
    session_id: String,
    user_only: bool,
    assistant_only: bool,
    format: String,
) -> Result<()> {
    // 冲突检测
    if user_only && assistant_only {
        eprintln!("Error: --user-only and --assistant-only are mutually exclusive");
        std::process::exit(1);
    }

    // format 校验
    let format: OutputFormat = match format.parse() {
        Ok(f) => f,
        Err(_) => {
            eprintln!("Error: invalid format '{}'. Valid options: text, json, markdown", format);
            std::process::exit(1);
        }
    };

    let manager = DataSourceManager::new();

    // 查找会话（支持前缀匹配）
    let sessions = manager.find_sessions(&session_id);

    let session = match sessions.len() {
        0 => {
            eprintln!("Session not found: {}", session_id);
            std::process::exit(1);
        }
        1 => sessions.into_iter().next().unwrap(),
        _ => {
            // 多个匹配，列出供用户选择
            eprintln!("Multiple sessions match '{}'. Please use a longer prefix:\n", session_id);
            for s in &sessions {
                let short_id = if s.id.len() > 12 { &s.id[..12] } else { &s.id };
                let project = s.project_name.as_deref().unwrap_or(&s.project_path);
                eprintln!("  {} [{}] {}", short_id, s.source, project);
            }
            std::process::exit(1);
        }
    };

    // 解析会话
    let parse_result = manager.parse_session(&session)?;
    let parse_result = match parse_result {
        Some(r) => r,
        None => {
            eprintln!("Failed to parse session: {}", session_id);
            std::process::exit(1);
        }
    };

    let output = output::format_session_detail(
        &session,
        &parse_result.messages,
        format,
        user_only,
        assistant_only,
    );
    println!("{}", output);

    Ok(())
}

/// 数据源命令
fn cmd_sources(format: String) -> Result<()> {
    // format 校验
    let format: OutputFormat = match format.parse() {
        Ok(f) => f,
        Err(_) => {
            eprintln!("Error: invalid format '{}'. Valid options: text, json", format);
            std::process::exit(1);
        }
    };

    let manager = DataSourceManager::new();
    let sources = manager.list_sources();

    let output = output::format_sources(&sources, format);
    println!("{}", output);

    Ok(())
}

/// 解析时间范围参数
/// 返回 (start_time_ms, end_time_ms)
fn parse_time_range(
    days: Option<u64>,
    since: Option<String>,
    until: Option<String>,
) -> (Option<u64>, Option<u64>) {
    use chrono::Local;

    let now = Local::now();

    // --days 优先级最高，会覆盖 --since
    let start_time = if let Some(d) = days {
        let start = now - chrono::Duration::days(d as i64);
        Some(start.timestamp_millis() as u64)
    } else if let Some(ref date_str) = since {
        parse_date_to_millis(date_str)
    } else {
        None
    };

    let end_time = until.as_ref().and_then(|s| parse_date_to_millis(s));

    (start_time, end_time)
}

/// 解析日期字符串为毫秒时间戳
fn parse_date_to_millis(date_str: &str) -> Option<u64> {
    use chrono::{Local, NaiveDate, TimeZone};

    // 尝试 YYYY-MM-DD 格式
    if let Ok(date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
        let datetime = date.and_hms_opt(0, 0, 0)?;
        let local = Local.from_local_datetime(&datetime).single()?;
        return Some(local.timestamp_millis() as u64);
    }

    // 尝试解析为天数
    if let Ok(days) = date_str.parse::<u64>() {
        let start = Local::now() - chrono::Duration::days(days as i64);
        return Some(start.timestamp_millis() as u64);
    }

    None
}
