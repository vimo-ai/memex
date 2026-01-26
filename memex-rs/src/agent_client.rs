//! Agent Client 封装
//!
//! 连接 vimo-agent 并订阅事件，用于事件驱动的 compact 触发。
//!
//! # 架构
//!
//! ```text
//! vimo-agent (文件监听 + 写入)
//!     │
//!     ├── Unix Socket IPC
//!     │
//!     ▼
//! memex-rs (Agent Client)
//!     │
//!     ├── 订阅 NewMessage 事件
//!     │
//!     ▼
//! CompactQueue (触发 compact)
//! ```

use ai_cli_session_db::{
    connect_or_start_agent, AgentClient, ClientConfig, EventType, Push,
};
use anyhow::Result;
use tokio::sync::mpsc;

/// Memex Agent Client
///
/// 封装与 vimo-agent 的连接，提供事件订阅功能。
pub struct MemexAgentClient {
    client: AgentClient,
}

impl MemexAgentClient {
    /// 连接到 Agent（如果 Agent 未运行则启动）
    pub async fn connect() -> Result<Self> {
        let config = ClientConfig::new("memex-rs");
        let mut client = connect_or_start_agent(config).await?;

        // 订阅 NewMessage 事件（用于触发 compact）
        client.subscribe(vec![EventType::NewMessage]).await?;

        tracing::info!("[AgentClient] Connected and subscribed to NewMessage event");

        Ok(Self { client })
    }

    /// 接收下一个推送事件
    pub async fn recv_event(&mut self) -> Option<Push> {
        self.client.recv_push().await
    }

    /// 获取内部 push 接收器（用于 tokio::select!）
    pub fn push_receiver(&mut self) -> &mut mpsc::Receiver<String> {
        self.client.push_receiver()
    }
}

/// NewMessage 事件处理器
///
/// 将 NewMessage 事件转换为 compact 任务。
#[derive(Debug, Clone)]
pub struct NewMessageEvent {
    /// 会话 ID
    pub session_id: String,
    /// 文件路径
    pub path: String,
    /// 新消息数量
    pub count: usize,
    /// 新消息 ID 列表
    pub message_ids: Vec<i64>,
}

impl NewMessageEvent {
    /// 从 Push 消息解析
    pub fn from_push(push: &Push) -> Option<Self> {
        match push {
            Push::NewMessages {
                session_id,
                path,
                count,
                message_ids,
            } => Some(Self {
                session_id: session_id.clone(),
                path: path.clone(),
                count: *count,
                message_ids: message_ids.clone(),
            }),
            _ => None,
        }
    }
}

/// 启动 Agent 事件循环
///
/// 监听 Agent 推送的事件，触发相应的处理逻辑。
///
/// # Arguments
///
/// * `compact_tx` - Compact 任务发送通道
/// * `index_tx` - 向量索引任务发送通道（可选）
pub async fn run_agent_event_loop(
    mut client: MemexAgentClient,
    compact_tx: Option<mpsc::Sender<NewMessageEvent>>,
    index_tx: Option<mpsc::Sender<Vec<i64>>>,
) {
    tracing::info!("[AgentEventLoop] Starting event loop");

    loop {
        match client.recv_event().await {
            Some(push) => {
                tracing::debug!("[AgentEventLoop] Received event: {:?}", push);

                if let Some(event) = NewMessageEvent::from_push(&push) {
                    tracing::debug!(
                        "[AgentEventLoop] NewMessage: session={}, count={}",
                        event.session_id,
                        event.count
                    );

                    // 触发向量索引（如果配置了）
                    if let Some(ref tx) = index_tx {
                        if !event.message_ids.is_empty() {
                            if let Err(e) = tx.send(event.message_ids.clone()).await {
                                tracing::error!("[AgentEventLoop] Failed to send index task: {}", e);
                            }
                        }
                    }

                    // 触发 compact（如果配置了）
                    if let Some(ref tx) = compact_tx {
                        if let Err(e) = tx.send(event).await {
                            tracing::error!("[AgentEventLoop] Failed to send compact task: {}", e);
                        }
                    }
                }
            }
            None => {
                tracing::warn!("[AgentEventLoop] Agent connection disconnected");
                break;
            }
        }
    }

    tracing::info!("[AgentEventLoop] Event loop ended");
}

/// 带重连的 Agent 事件循环
///
/// 当连接断开时自动重连。
pub async fn run_agent_event_loop_with_reconnect(
    compact_tx: Option<mpsc::Sender<NewMessageEvent>>,
    index_tx: Option<mpsc::Sender<Vec<i64>>>,
) {
    loop {
        match MemexAgentClient::connect().await {
            Ok(client) => {
                run_agent_event_loop(client, compact_tx.clone(), index_tx.clone()).await;
            }
            Err(e) => {
                tracing::error!("[AgentEventLoop] Failed to connect to Agent: {}", e);
            }
        }

        // 断开后等待 5 秒再重连
        tracing::info!("[AgentEventLoop] Reconnecting in 5 seconds...");
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
    }
}
