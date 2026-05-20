use anyhow::Result;
use tracing::{debug, info};

use crate::llm::{ChatProvider, ChatProviderExt, EmbeddingProvider};

use super::prompt;
use super::store::{KnowledgeCluster, KnowledgeNode, KnowledgeRelation};

/// Match result: (node_index, cluster_index, similarity)
pub type MatchHit = (usize, usize, f64);

/// Embedding text for a node: "topic: conclusion"
fn node_embed_text(node: &KnowledgeNode) -> String {
    format!("{}: {}", node.topic, node.conclusion)
}

fn cluster_embed_text(cluster: &KnowledgeCluster) -> String {
    format!(
        "{}: {}",
        cluster.canonical_topic,
        cluster.current_understanding.as_deref().unwrap_or(&cluster.canonical_topic)
    )
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;
    for (x, y) in a.iter().zip(b.iter()) {
        let x = *x as f64;
        let y = *y as f64;
        dot += x * y;
        norm_a += x * x;
        norm_b += y * y;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < 1e-10 {
        0.0
    } else {
        dot / denom
    }
}

/// Match new nodes against existing clusters by embedding similarity.
pub async fn match_nodes_to_clusters(
    embed: &dyn EmbeddingProvider,
    new_nodes: &[KnowledgeNode],
    existing_clusters: &[KnowledgeCluster],
    threshold: f64,
) -> Result<(Vec<MatchHit>, Vec<usize>)> {
    if new_nodes.is_empty() {
        return Ok((vec![], vec![]));
    }
    if existing_clusters.is_empty() {
        let unmatched: Vec<usize> = (0..new_nodes.len()).collect();
        return Ok((vec![], unmatched));
    }

    let node_texts: Vec<String> = new_nodes.iter().map(node_embed_text).collect();
    let cluster_texts: Vec<String> = existing_clusters.iter().map(cluster_embed_text).collect();

    info!("embedding {} nodes + {} clusters", node_texts.len(), cluster_texts.len());

    let node_embs = embed.embed_batch(node_texts).await?;
    let cluster_embs = embed.embed_batch(cluster_texts).await?;

    let mut matched = Vec::new();
    let mut unmatched = Vec::new();

    for (ni, node_emb) in node_embs.iter().enumerate() {
        let mut best_ci = 0;
        let mut best_sim = f64::NEG_INFINITY;
        for (ci, cluster_emb) in cluster_embs.iter().enumerate() {
            let sim = cosine_similarity(node_emb, cluster_emb);
            if sim > best_sim {
                best_sim = sim;
                best_ci = ci;
            }
        }
        if best_sim >= threshold {
            matched.push((ni, best_ci, best_sim));
        } else {
            unmatched.push(ni);
        }
    }

    debug!("{} matched, {} unmatched (threshold {threshold})", matched.len(), unmatched.len());
    Ok((matched, unmatched))
}

/// Cluster unmatched nodes among themselves using average-linkage.
/// Returns (multi_node_clusters, singleton_indices).
pub async fn cluster_unmatched(
    embed: &dyn EmbeddingProvider,
    nodes: &[KnowledgeNode],
    unmatched_indices: &[usize],
    threshold: f64,
) -> Result<(Vec<Vec<usize>>, Vec<usize>)> {
    if unmatched_indices.is_empty() {
        return Ok((vec![], vec![]));
    }

    // embeddings already computed in match phase — but we don't cache them yet
    // for simplicity, re-embed the unmatched subset
    let texts: Vec<String> = unmatched_indices
        .iter()
        .map(|&i| node_embed_text(&nodes[i]))
        .collect();
    let embs = embed.embed_batch(texts).await?;

    // precompute pairwise similarity
    let n = unmatched_indices.len();
    let mut sim_matrix = vec![vec![0.0f64; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            let s = cosine_similarity(&embs[i], &embs[j]);
            sim_matrix[i][j] = s;
            sim_matrix[j][i] = s;
        }
        sim_matrix[i][i] = 1.0;
    }

    // greedy average-linkage clustering
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for pos in 0..n {
        let ni = unmatched_indices[pos];
        let mut best_cluster = None;
        let mut best_avg = 0.0f64;

        for (ci, members) in clusters.iter().enumerate() {
            let avg_sim: f64 = members
                .iter()
                .map(|&mi| {
                    let mi_pos = unmatched_indices.iter().position(|&x| x == mi).unwrap();
                    sim_matrix[pos][mi_pos]
                })
                .sum::<f64>()
                / members.len() as f64;

            if avg_sim >= threshold && avg_sim > best_avg {
                best_cluster = Some(ci);
                best_avg = avg_sim;
            }
        }

        if let Some(ci) = best_cluster {
            clusters[ci].push(ni);
        } else {
            clusters.push(vec![ni]);
        }
    }

    let multi: Vec<Vec<usize>> = clusters.iter().filter(|c| c.len() > 1).cloned().collect();
    let singletons: Vec<usize> = clusters
        .iter()
        .filter(|c| c.len() == 1)
        .map(|c| c[0])
        .collect();

    debug!("{} multi-clusters, {} singletons", multi.len(), singletons.len());
    Ok((multi, singletons))
}

/// LLM-name new clusters in batch.
pub async fn name_clusters(
    chat: &dyn ChatProvider,
    clusters: &[Vec<usize>],
    nodes: &[KnowledgeNode],
) -> Result<Vec<String>> {
    if clusters.is_empty() {
        return Ok(vec![]);
    }

    let mut lines = Vec::new();
    for (i, members) in clusters.iter().enumerate() {
        let topics: Vec<&str> = members
            .iter()
            .take(6)
            .map(|&ni| nodes[ni].topic.as_str())
            .collect();
        lines.push(format!("C{i}: {}", topics.join(", ")));
    }

    let prompt = format!(
        "Name each cluster below with a canonical topic name (2-5 words, English).\n\n{}\n\nReply with one name per line, format \"C0: Name Here\", nothing else. /no_think",
        lines.join("\n")
    );

    let resp = chat.chat_simple(&prompt).await?;

    let mut names: Vec<Option<String>> = vec![None; clusters.len()];
    for line in resp.content.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix('C') {
            if let Some((idx_str, name)) = rest.split_once(':') {
                if let Ok(idx) = idx_str.trim().parse::<usize>() {
                    if idx < clusters.len() {
                        let name = name.trim().trim_matches('"').trim_matches('\'').to_string();
                        if !name.is_empty() {
                            names[idx] = Some(name);
                        }
                    }
                }
            }
        }
    }

    // fallback to first node's topic
    let result: Vec<String> = clusters
        .iter()
        .enumerate()
        .map(|(i, members)| {
            names[i]
                .clone()
                .unwrap_or_else(|| nodes[members[0]].topic.clone())
        })
        .collect();

    Ok(result)
}

/// Run evolution analysis on a cluster with enough history.
/// Returns (current_understanding, evolution_narrative, final_confidence, relations).
pub async fn evolve_cluster(
    chat: &dyn ChatProvider,
    cluster: &KnowledgeCluster,
    nodes: &[KnowledgeNode],
    project_desc: &str,
) -> Result<Option<EvolutionResult>> {
    const MIN_NODES: usize = 3;
    const MIN_DAYS: usize = 2;

    if nodes.len() < MIN_NODES {
        return Ok(None);
    }

    let unique_days: std::collections::HashSet<&str> = nodes.iter().map(|n| n.day.as_str()).collect();
    if unique_days.len() < MIN_DAYS {
        return Ok(None);
    }

    let mut node_lines = Vec::new();
    for (i, node) in nodes.iter().enumerate() {
        node_lines.push(format!(
            "N{i} [{}] [{}|{}] {}: {}",
            node.day, node.node_type, node.confidence, node.topic, node.conclusion
        ));
    }

    let prompt_text = prompt::format_evolve(
        &cluster.canonical_topic,
        &node_lines.join("\n"),
        project_desc,
    );

    let resp = chat
        .chat_with_system(prompt::SYSTEM_PROMPT, &prompt_text)
        .await?;

    let parsed: EvolutionOutput = match prompt::try_parse_json(&resp.content) {
        Some(v) => v,
        None => return Ok(None),
    };

    let relations: Vec<KnowledgeRelation> = parsed
        .relationships
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| {
            let from_idx = r.from.strip_prefix('N')?.parse::<usize>().ok()?;
            let to_idx = r.to.strip_prefix('N')?.parse::<usize>().ok()?;
            let from_node = nodes.get(from_idx)?;
            let to_node = nodes.get(to_idx)?;
            Some(KnowledgeRelation {
                from_node_id: from_node.id.clone(),
                to_node_id: to_node.id.clone(),
                relation_type: r.r#type,
                detail: r.detail,
            })
        })
        .collect();

    Ok(Some(EvolutionResult {
        current_understanding: parsed.current_understanding,
        evolution_narrative: parsed.evolution_narrative,
        final_confidence: parsed.final_confidence,
        relations,
    }))
}

// ==================== Types ====================

pub struct EvolutionResult {
    pub current_understanding: String,
    pub evolution_narrative: String,
    pub final_confidence: f64,
    pub relations: Vec<KnowledgeRelation>,
}

#[derive(serde::Deserialize)]
struct EvolutionOutput {
    current_understanding: String,
    evolution_narrative: String,
    final_confidence: f64,
    relationships: Option<Vec<RawRelation>>,
}

#[derive(serde::Deserialize)]
struct RawRelation {
    from: String,
    to: String,
    r#type: String,
    detail: Option<String>,
}

// ==================== Tests ====================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_similarity_identical() {
        let a = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &a);
        assert!((sim - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_opposite() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![-1.0, -2.0, -3.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim + 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_similarity_zero_vector() {
        let a = vec![0.0, 0.0, 0.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }
}
