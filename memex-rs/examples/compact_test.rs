//! Compact 真实数据测试
//!
//! 用法:
//! cargo run --example compact_test              # 默认用 Ollama qwen3:0.6b
//! OLLAMA_MODEL=qwen3:8b cargo run --example compact_test  # 指定模型

use std::sync::Arc;
use anyhow::Result;

use memex::{
    compact::{build_parsed_session, CompactConfig, CompactDB, CompactService},
    llm::{ChatProviderExt, OllamaProvider},
    shared_adapter::SharedDbAdapter,
};

const TEST_SESSION_ID: &str = "170de28f-cb5b-4d21-a50d-e0d998543ead";

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_env_filter("info,memex=debug")
        .init();

    // 1. 创建 Ollama Provider
    let model = std::env::var("OLLAMA_MODEL").unwrap_or_else(|_| "qwen3:0.6b".to_string());
    println!("📡 使用 Ollama 本地模型: {}", model);

    let chat_provider: Arc<dyn memex::llm::ChatProvider> =
        Arc::new(OllamaProvider::new("http://localhost:11434", &model, &model));

    // 3. 测试 LLM 连通性
    println!("\n🔌 测试 LLM 连接...");
    let test_response = chat_provider.chat_simple("Say 'OK' if you can hear me.").await?;
    println!("   LLM 响应: {}", test_response.content.trim());

    // 4. 连接数据库
    let db_path = dirs::home_dir()
        .unwrap()
        .join(".vimo/db/ai-cli-session.db");
    println!("\n📂 数据库: {:?}", db_path);

    let shared_db = Arc::new(SharedDbAdapter::new(Some(db_path))?);

    // 5. 创建 Compact DB（使用临时目录）
    let compact_db_path = std::env::temp_dir().join("compact_test.db");
    println!("📦 Compact DB: {:?}", compact_db_path);
    let compact_db = Arc::new(CompactDB::connect(&compact_db_path, Some("trigram"))?);

    // 6. 获取测试 session 的消息
    println!("\n📥 加载 session: {}", TEST_SESSION_ID);
    let messages = shared_db.get_messages(TEST_SESSION_ID).await?;
    println!("   消息数: {}", messages.len());

    // 7. 解析成 ParsedSession
    let parsed = build_parsed_session(TEST_SESSION_ID, messages, 0);
    println!("   对话轮数: {}", parsed.talks.len());
    println!("   工具调用: {}", parsed.tool_calls.len());

    // 打印每轮对话概要
    println!("\n📋 对话概要:");
    for talk in &parsed.talks {
        let user_preview: String = talk.user_message.content.chars().take(50).collect();
        let assistant_preview: String = talk.assistant_messages
            .first()
            .map(|m| m.content.chars().take(30).collect())
            .unwrap_or_else(|| "(empty)".to_string());
        println!(
            "   #{}: {} (assistant: {} msgs [{}...], tools: {})",
            talk.prompt_number,
            user_preview,
            talk.assistant_messages.len(),
            assistant_preview,
            talk.tool_calls.len()
        );
    }

    // 8. 创建 CompactService 并处理
    println!("\n🔄 开始 Compact 处理...");
    let config = CompactConfig {
        l1_observations: true,
        l2_talk_summary: true,
        l3_session_summary: false, // 先不测 L3
        ..Default::default()
    };

    let service = CompactService::new(
        shared_db.clone(),
        chat_provider.clone(),
        compact_db.clone(),
        config,
    );

    let result = service.process_session(TEST_SESSION_ID).await?;

    println!("\n✅ Compact 完成:");
    println!("   L1 Observations: {}", result.observations_count);
    println!("   L2 Talk Summaries: {}", result.talk_summaries_count);
    println!("   工具调用剪枝: {}", result.tool_calls_pruned);
    println!("   工具调用合并: {}", result.tool_calls_merged);

    // 9. 查看生成的结果
    if result.talk_summaries_count > 0 {
        println!("\n📝 L2 Talk Summaries:");
        let summaries = compact_db.get_talk_summaries(TEST_SESSION_ID).await?;
        for s in summaries {
            println!("   #{}: {}", s.prompt_number, s.summary);
        }
    }

    println!("\n🎉 测试完成!");
    Ok(())
}
