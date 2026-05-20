use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Result};
use sha2::{Digest, Sha256};
use tracing::{info, warn};

use crate::db_reader::DbReader;
use crate::llm::{ChatProvider, EmbeddingProvider};

use super::config::KnowledgeConfig;
use super::extractor::Extractor;
use super::matcher;
use super::store::{KnowledgeCluster, KnowledgeStore};

pub struct KnowledgeService {
    store: KnowledgeStore,
    extractor: Extractor,
    chat: Arc<dyn ChatProvider>,
    embed: Arc<dyn EmbeddingProvider>,
    config: KnowledgeConfig,
}

#[derive(Debug, Default)]
pub struct ProcessResult {
    pub sessions_processed: usize,
    pub sessions_failed: usize,
    pub nodes_extracted: usize,
    pub nodes_matched: usize,
    pub clusters_created: usize,
    pub clusters_evolved: usize,
}

impl std::fmt::Display for ProcessResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "processed {}, failed {}, nodes {} (matched {}), clusters +{}, evolved {}",
            self.sessions_processed,
            self.sessions_failed,
            self.nodes_extracted,
            self.nodes_matched,
            self.clusters_created,
            self.clusters_evolved,
        )
    }
}

impl KnowledgeService {
    pub fn new(
        store: KnowledgeStore,
        db: Arc<DbReader>,
        chat: Arc<dyn ChatProvider>,
        embed: Arc<dyn EmbeddingProvider>,
        config: KnowledgeConfig,
    ) -> Self {
        Self {
            store,
            extractor: Extractor::new(db),
            chat,
            embed,
            config,
        }
    }

    pub async fn ensure_schema(&self) -> Result<()> {
        self.store.ensure_schema().await
    }

    /// Process specific sessions through the full pipeline.
    pub async fn process_sessions(
        &self,
        sessions: &[(String, i64, String)], // (session_id, project_id, day)
        project_desc: &str,
    ) -> Result<ProcessResult> {
        self.process_sessions_with_progress(sessions, project_desc, |_, _| {}, None).await
    }

    /// Process sessions with per-session progress callback and optional cancel flag.
    /// Callback receives (sessions_done, &ProcessResult) after each session.
    pub async fn process_sessions_with_progress<F>(
        &self,
        sessions: &[(String, i64, String)],
        project_desc: &str,
        on_progress: F,
        cancel: Option<&AtomicBool>,
    ) -> Result<ProcessResult>
    where
        F: Fn(usize, &ProcessResult),
    {
        let mut result = ProcessResult::default();
        let mut session_ids_done: Vec<String> = Vec::new();

        // Stage 1: Extract + persist nodes per session (atomic per session)
        for (session_id, project_id, day) in sessions {
            if cancel.map_or(false, |c| c.load(Ordering::Relaxed)) {
                info!("extraction cancelled after {} sessions", result.sessions_processed);
                bail!("cancelled");
            }
            info!("extracting session {}...", &session_id[..8.min(session_id.len())]);

            let chunks = match self
                .extractor
                .extract_conversation_digest(session_id, self.config.max_chunk_chars)
                .await
            {
                Ok(c) => c,
                Err(e) => {
                    warn!("digest failed for {}: {e}", &session_id[..8.min(session_id.len())]);
                    self.store
                        .update_progress(session_id, "failed", 0, Some(&e.to_string()), &self.config.pipeline_version)
                        .await?;
                    result.sessions_failed += 1;
                    on_progress(result.sessions_processed + result.sessions_failed, &result);
                    continue;
                }
            };

            match self
                .extractor
                .extract_nodes(
                    self.chat.as_ref(),
                    session_id,
                    *project_id,
                    day,
                    &chunks,
                    project_desc,
                    &self.config.pipeline_version,
                )
                .await
            {
                Ok(nodes) => {
                    let expected = nodes.len();
                    let written = self.store.insert_nodes(&nodes).await?;
                    if written == 0 && expected > 0 {
                        let sid = &session_id[..8.min(session_id.len())];
                        warn!("insert_nodes wrote 0/{expected} for {sid} — possible schema constraint issue");
                        self.store
                            .update_progress(session_id, "failed", 0, Some("insert_nodes wrote 0 rows"), &self.config.pipeline_version)
                            .await?;
                        result.sessions_failed += 1;
                    } else {
                        self.store
                            .update_progress(session_id, "extracted", written as i64, None, &self.config.pipeline_version)
                            .await?;
                        result.sessions_processed += 1;
                        result.nodes_extracted += written;
                        session_ids_done.push(session_id.clone());
                    }
                }
                Err(e) => {
                    warn!("extraction failed for {}: {e}", &session_id[..8.min(session_id.len())]);
                    self.store
                        .update_progress(session_id, "failed", 0, Some(&e.to_string()), &self.config.pipeline_version)
                        .await?;
                    result.sessions_failed += 1;
                }
            }
            on_progress(result.sessions_processed + result.sessions_failed, &result);
        }

        if session_ids_done.is_empty() {
            return Ok(result);
        }

        // Stage 2: Read back persisted nodes, match against existing clusters
        let all_nodes = self.store.get_nodes_by_sessions(&session_ids_done).await?;
        let project_ids: Vec<i64> = sessions.iter().map(|(_, pid, _)| *pid).collect::<HashSet<_>>().into_iter().collect();
        let existing_clusters = self.store.load_clusters(&project_ids).await?;

        let (matched, unmatched) = matcher::match_nodes_to_clusters(
            self.embed.as_ref(),
            &all_nodes,
            &existing_clusters,
            self.config.match_threshold,
        )
        .await?;

        result.nodes_matched = matched.len();
        let mut affected_cluster_ids: HashSet<String> = HashSet::new();

        // Assign matched nodes to existing clusters
        for &(ni, ci, _sim) in &matched {
            let node = &all_nodes[ni];
            let cluster_id = &existing_clusters[ci].id;
            self.store.update_node_cluster(&node.id, cluster_id).await?;
            affected_cluster_ids.insert(cluster_id.clone());
        }

        // Stage 3: Cluster unmatched nodes
        let (multi_clusters, singleton_indices) = matcher::cluster_unmatched(
            self.embed.as_ref(),
            &all_nodes,
            &unmatched,
            self.config.cluster_threshold,
        )
        .await?;

        // Name new multi-node clusters
        let cluster_names = matcher::name_clusters(self.chat.as_ref(), &multi_clusters, &all_nodes).await?;

        // Create new clusters for multi-node groups
        for (ci, members) in multi_clusters.iter().enumerate() {
            let cluster_id = cluster_id_from_node_ids(members.iter().map(|&i| &all_nodes[i].id));
            let project_id = all_nodes[members[0]].project_id;
            let name = &cluster_names[ci];

            let cluster = KnowledgeCluster {
                id: cluster_id.clone(),
                project_id,
                canonical_topic: name.clone(),
                l5_domain: None,
                current_understanding: None,
                evolution_narrative: None,
                final_confidence: None,
                node_count: members.len() as i64,
                day_count: 0,
                first_day: None,
                last_day: None,
                pipeline_version: Some(self.config.pipeline_version.clone()),
            };
            self.store.insert_cluster(&cluster).await?;
            for &ni in members {
                self.store.update_node_cluster(&all_nodes[ni].id, &cluster_id).await?;
            }
            affected_cluster_ids.insert(cluster_id);
            result.clusters_created += 1;
        }

        // Create singleton clusters
        for &ni in &singleton_indices {
            let node = &all_nodes[ni];
            let cluster_id = cluster_id_from_node_ids(std::iter::once(&node.id));
            let cluster = KnowledgeCluster {
                id: cluster_id.clone(),
                project_id: node.project_id,
                canonical_topic: node.topic.clone(),
                l5_domain: None,
                current_understanding: None,
                evolution_narrative: None,
                final_confidence: None,
                node_count: 1,
                day_count: 1,
                first_day: Some(node.day.clone()),
                last_day: Some(node.day.clone()),
                pipeline_version: Some(self.config.pipeline_version.clone()),
            };
            self.store.insert_cluster(&cluster).await?;
            self.store.update_node_cluster(&node.id, &cluster_id).await?;
            result.clusters_created += 1;
        }

        // Update stats for affected clusters
        for cid in &affected_cluster_ids {
            self.store.update_cluster_stats(cid).await?;
        }

        // Stage 4: Evolve clusters with enough history
        for cid in &affected_cluster_ids {
            let nodes = self.store.get_nodes_by_cluster(cid).await?;
            let clusters = self.store.load_clusters(&project_ids).await?;
            let cluster = match clusters.iter().find(|c| &c.id == cid) {
                Some(c) => c,
                None => continue,
            };

            match matcher::evolve_cluster(self.chat.as_ref(), cluster, &nodes, project_desc).await {
                Ok(Some(evo)) => {
                    self.store
                        .update_cluster_evolution(
                            cid,
                            &evo.current_understanding,
                            &evo.evolution_narrative,
                            evo.final_confidence,
                        )
                        .await?;
                    if !evo.relations.is_empty() {
                        self.store.upsert_relations(&evo.relations).await?;
                    }
                    result.clusters_evolved += 1;
                }
                Ok(None) => {}
                Err(e) => {
                    warn!("evolve failed for cluster {cid}: {e}");
                }
            }
        }

        info!("pipeline done: {result}");
        Ok(result)
    }

    /// Process all unprocessed sessions for given project IDs.
    pub async fn process_project(
        &self,
        project_ids: &[i64],
        limit: usize,
        project_desc: &str,
    ) -> Result<ProcessResult> {
        self.process_project_with_progress(project_ids, limit, project_desc, |_, _| {}, None).await
    }

    /// Process unprocessed sessions with progress callback and optional cancel flag.
    pub async fn process_project_with_progress<F>(
        &self,
        project_ids: &[i64],
        limit: usize,
        project_desc: &str,
        on_progress: F,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<ProcessResult>
    where
        F: Fn(usize, &ProcessResult),
    {
        let sessions = self.store.get_unprocessed_sessions(project_ids, limit).await?;
        if sessions.is_empty() {
            info!("no unprocessed sessions");
            return Ok(ProcessResult::default());
        }
        info!("{} unprocessed sessions (limit {limit})", sessions.len());
        self.process_sessions_with_progress(&sessions, project_desc, on_progress, cancel.as_deref()).await
    }

    /// Count unprocessed sessions for given project IDs.
    pub async fn count_unprocessed(&self, project_ids: &[i64], limit: usize) -> Result<usize> {
        let sessions = self.store.get_unprocessed_sessions(project_ids, limit).await?;
        Ok(sessions.len())
    }

    /// Query knowledge nodes associated with given session IDs (for MCP search_history).
    pub async fn get_knowledge_for_sessions(
        &self,
        session_ids: &[String],
    ) -> Result<Vec<SessionKnowledge>> {
        let nodes = self.store.get_nodes_by_sessions(session_ids).await?;
        if nodes.is_empty() {
            return Ok(vec![]);
        }

        let cluster_ids: Vec<&str> = nodes.iter().map(|n| n.cluster_id.as_str()).collect::<HashSet<_>>().into_iter().collect();
        // load clusters in bulk — reuse project_ids from nodes
        let project_ids: Vec<i64> = nodes.iter().map(|n| n.project_id).collect::<HashSet<_>>().into_iter().collect();
        let all_clusters = self.store.load_clusters(&project_ids).await?;

        let mut results = Vec::new();
        for node in &nodes {
            let cluster = all_clusters.iter().find(|c| c.id == node.cluster_id);
            results.push(SessionKnowledge {
                session_id: node.session_id.clone(),
                topic: node.topic.clone(),
                conclusion: node.conclusion.clone(),
                confidence: node.confidence,
                cluster_topic: cluster.map(|c| c.canonical_topic.clone()),
                cluster_size: cluster.map(|c| c.node_count),
            });
        }

        // suppress unused variable warning
        let _ = cluster_ids;
        Ok(results)
    }

    pub async fn stats(&self, project_ids: &[i64]) -> Result<(i64, i64)> {
        self.store.stats(project_ids).await
    }

    pub async fn global_stats(&self) -> Result<(i64, i64)> {
        self.store.global_stats().await
    }

    pub async fn get_pending_projects(
        &self,
        search: Option<&str>,
        limit: usize,
    ) -> Result<Vec<super::store::PendingProject>> {
        self.store.get_pending_projects(search, limit).await
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionKnowledge {
    pub session_id: String,
    pub topic: String,
    pub conclusion: String,
    pub confidence: f64,
    pub cluster_topic: Option<String>,
    pub cluster_size: Option<i64>,
}

fn cluster_id_from_node_ids<'a>(ids: impl Iterator<Item = &'a String>) -> String {
    let mut sorted: Vec<&str> = ids.map(|s| s.as_str()).collect();
    sorted.sort();
    let raw = sorted.join("|");
    let hash = Sha256::digest(raw.as_bytes());
    hash[..8].iter().map(|b| format!("{b:02x}")).collect()
}
