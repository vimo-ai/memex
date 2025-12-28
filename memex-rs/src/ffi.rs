//! FFI 接口 - 供 Swift/C 调用
//!
//! 提供 C ABI 接口，用于 ETerm 的 MemexKit 插件

use std::ffi::{c_char, c_int, CStr, CString};
use std::path::Path;
use std::ptr;

use ai_cli_session_collector::{ClaudeAdapter, ConversationAdapter};

use crate::db::Database;
use crate::domain::SearchResult;

/// Memex 句柄
pub struct MemexHandle {
    db: Database,
    claude_adapter: ClaudeAdapter,
}

/// 搜索结果（C 兼容）
#[repr(C)]
pub struct CSearchResult {
    pub message_id: i64,
    pub session_id: *mut c_char,
    pub project_id: i64,
    pub project_name: *mut c_char,
    pub message_type: *mut c_char,
    pub content: *mut c_char,
    pub snippet: *mut c_char,
    pub score: f64,
    pub timestamp: *mut c_char,
}

/// 搜索结果列表（C 兼容）
#[repr(C)]
pub struct CSearchResults {
    pub results: *mut CSearchResult,
    pub count: usize,
}

/// 会话详情（C 兼容）
#[repr(C)]
pub struct CSession {
    pub id: *mut c_char,
    pub project_id: i64,
    pub project_name: *mut c_char,
    pub message_count: i64,
    pub first_message: *mut c_char,
    pub last_message: *mut c_char,
}

// ==================== 初始化 ====================

/// 初始化 Memex
///
/// - `data_dir`: 数据目录路径（如 ~/.memex-data）
///
/// 返回 MemexHandle 指针，失败返回 null
#[no_mangle]
pub extern "C" fn memex_init(data_dir: *const c_char) -> *mut MemexHandle {
    let data_dir = match unsafe { CStr::from_ptr(data_dir) }.to_str() {
        Ok(s) => s,
        Err(_) => return ptr::null_mut(),
    };

    let db_path = Path::new(data_dir).join("memex.db");
    let db = match Database::open(&db_path) {
        Ok(db) => db,
        Err(e) => {
            eprintln!("[memex-ffi] 打开数据库失败: {}", e);
            return ptr::null_mut();
        }
    };

    // Claude projects 路径
    let home = dirs::home_dir().unwrap_or_default();
    let claude_projects = home.join(".claude").join("projects");
    let claude_adapter = ClaudeAdapter::new(claude_projects);

    let handle = Box::new(MemexHandle { db, claude_adapter });
    Box::into_raw(handle)
}

/// 释放 Memex 句柄
#[no_mangle]
pub extern "C" fn memex_free(handle: *mut MemexHandle) {
    if !handle.is_null() {
        unsafe {
            let _ = Box::from_raw(handle);
        }
    }
}

// ==================== 索引 ====================

/// 索引单个 JSONL 文件
///
/// - `handle`: Memex 句柄
/// - `jsonl_path`: JSONL 文件路径
/// - `project_path`: 项目路径（解码后的真实路径）
///
/// 返回插入的消息数量，失败返回 -1
#[no_mangle]
pub extern "C" fn memex_index_jsonl(
    handle: *mut MemexHandle,
    jsonl_path: *const c_char,
    project_path: *const c_char,
) -> c_int {
    let handle = match unsafe { handle.as_ref() } {
        Some(h) => h,
        None => return -1,
    };

    let jsonl_path = match unsafe { CStr::from_ptr(jsonl_path) }.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };

    let project_path = match unsafe { CStr::from_ptr(project_path) }.to_str() {
        Ok(s) => s,
        Err(_) => return -1,
    };

    // 从文件名提取 session_id
    let session_id = Path::new(jsonl_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    if session_id.is_empty() {
        return -1;
    }

    // 提取项目名
    let project_name = project_path
        .split('/')
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or(project_path);

    // 获取或创建项目
    let project_id = match handle.db.get_or_create_project(project_name, project_path, "claude") {
        Ok(id) => id,
        Err(_) => return -1,
    };

    // 构造 SessionMeta
    let meta = ai_cli_session_collector::SessionMeta {
        id: session_id.to_string(),
        source: ai_cli_session_collector::Source::Claude,
        channel: Some("code".to_string()),
        project_path: project_path.to_string(),
        project_name: Some(project_name.to_string()),
        encoded_dir_name: None,
        session_path: Some(jsonl_path.to_string()),
        file_mtime: None,
        file_size: None,
        cwd: None,
        model: None,
        meta: None,
        created_at: None,
        updated_at: None,
    };

    // 解析会话
    let parse_result = match handle.claude_adapter.parse_session(&meta) {
        Ok(Some(r)) => r,
        Ok(None) => return 0,
        Err(_) => return -1,
    };

    // 创建会话
    if let Err(_) = handle.db.create_session_v2(
        session_id,
        project_id,
        parse_result.cwd.as_deref(),
        parse_result.model.as_deref(),
        "claude",
        Some("code"),
    ) {
        return -1;
    }

    // 插入消息
    match handle.db.insert_messages_v2(session_id, &parse_result.messages) {
        Ok((inserted, _)) => inserted as c_int,
        Err(_) => -1,
    }
}

// ==================== 搜索 ====================

/// FTS 全文搜索
///
/// - `handle`: Memex 句柄
/// - `query`: 搜索查询
/// - `limit`: 返回数量限制
///
/// 返回 CSearchResults 指针，失败返回 null
#[no_mangle]
pub extern "C" fn memex_search(
    handle: *mut MemexHandle,
    query: *const c_char,
    limit: u32,
) -> *mut CSearchResults {
    let handle = match unsafe { handle.as_ref() } {
        Some(h) => h,
        None => return ptr::null_mut(),
    };

    let query = match unsafe { CStr::from_ptr(query) }.to_str() {
        Ok(s) => s,
        Err(_) => return ptr::null_mut(),
    };

    let results = match handle.db.search(query, limit as usize, None) {
        Ok(r) => r,
        Err(_) => return ptr::null_mut(),
    };

    let c_results = results_to_c(&results);
    Box::into_raw(Box::new(c_results))
}

/// 释放搜索结果
#[no_mangle]
pub extern "C" fn memex_free_search_results(results: *mut CSearchResults) {
    if results.is_null() {
        return;
    }

    unsafe {
        let results = Box::from_raw(results);
        if !results.results.is_null() {
            let slice = std::slice::from_raw_parts_mut(results.results, results.count);
            for item in slice.iter_mut() {
                free_c_string(item.session_id);
                free_c_string(item.project_name);
                free_c_string(item.message_type);
                free_c_string(item.content);
                free_c_string(item.snippet);
                free_c_string(item.timestamp);
            }
            let _ = Vec::from_raw_parts(results.results, results.count, results.count);
        }
    }
}

// ==================== 会话 ====================

/// 获取会话详情
///
/// - `handle`: Memex 句柄
/// - `session_id`: 会话 ID
///
/// 返回 CSession 指针，失败返回 null
#[no_mangle]
pub extern "C" fn memex_get_session(
    handle: *mut MemexHandle,
    session_id: *const c_char,
) -> *mut CSession {
    let handle = match unsafe { handle.as_ref() } {
        Some(h) => h,
        None => return ptr::null_mut(),
    };

    let session_id = match unsafe { CStr::from_ptr(session_id) }.to_str() {
        Ok(s) => s,
        Err(_) => return ptr::null_mut(),
    };

    let session = match handle.db.get_session(session_id) {
        Ok(Some(s)) => s,
        _ => return ptr::null_mut(),
    };

    let c_session = CSession {
        id: to_c_string(&session.id),
        project_id: session.project_id,
        project_name: to_c_string(&session.project_name),
        message_count: session.message_count,
        first_message: session.first_message.map(|s| to_c_string(&s)).unwrap_or(ptr::null_mut()),
        last_message: session.last_message.map(|s| to_c_string(&s)).unwrap_or(ptr::null_mut()),
    };

    Box::into_raw(Box::new(c_session))
}

/// 释放会话详情
#[no_mangle]
pub extern "C" fn memex_free_session(session: *mut CSession) {
    if session.is_null() {
        return;
    }

    unsafe {
        let session = Box::from_raw(session);
        free_c_string(session.id);
        free_c_string(session.project_name);
        free_c_string(session.first_message);
        free_c_string(session.last_message);
    }
}

// ==================== 辅助函数 ====================

fn to_c_string(s: &str) -> *mut c_char {
    CString::new(s).map(|cs| cs.into_raw()).unwrap_or(ptr::null_mut())
}

fn free_c_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            let _ = CString::from_raw(s);
        }
    }
}

fn results_to_c(results: &[SearchResult]) -> CSearchResults {
    let mut c_items: Vec<CSearchResult> = results
        .iter()
        .map(|r| CSearchResult {
            message_id: r.message_id,
            session_id: to_c_string(&r.session_id),
            project_id: r.project_id,
            project_name: to_c_string(&r.project_name),
            message_type: to_c_string(&r.r#type),
            content: to_c_string(&r.content),
            snippet: to_c_string(&r.snippet),
            score: r.score,
            timestamp: r.timestamp.as_ref().map(|s| to_c_string(s)).unwrap_or(ptr::null_mut()),
        })
        .collect();

    let ptr = c_items.as_mut_ptr();
    let count = c_items.len();
    std::mem::forget(c_items);

    CSearchResults {
        results: ptr,
        count,
    }
}
