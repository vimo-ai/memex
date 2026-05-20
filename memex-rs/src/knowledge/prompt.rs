pub const SYSTEM_PROMPT: &str =
    "You are a precise knowledge extraction engine. Output valid JSON only.";

pub const PASS1_PROMPT: &str = r#"You are analyzing a Claude Code conversation session to extract knowledge.
The conversation is from {project_desc}.

Below is a conversation digest - user messages and assistant response summaries.

For this session, analyze:
1. WHAT: What was the main goal and what was accomplished?
2. KEYWORDS: Technical terms, tools, APIs, algorithms mentioned
3. VALUABLE: What discoveries, decisions, or pitfalls emerged?
4. WORTH_EXTRACTING: Which specific conclusions could be useful for someone working on the same project later?

Be specific and technical. Focus on factual findings, not process descriptions.

=== CONVERSATION ===
{conversation}
=== END ===

Analyze this session. /no_think"#;

pub const PASS2_PROMPT: &str = r#"Based on the analysis below, extract structured knowledge nodes.

Each knowledge node should be:
- A specific technical fact, discovery, decision, or pitfall
- Something that would help a future developer working on the same project
- NOT a process description ("we tried X") but a conclusion ("X works because Y")

Output ONLY a JSON object with this schema:
{{
  "summary": "one-line session summary",
  "keywords": ["keyword1", "keyword2", ...],
  "knowledge_nodes": [
    {{
      "topic": "short topic name",
      "conclusion": "the specific finding or decision",
      "type": "discovery|decision|pitfall|technique",
      "confidence": 0.0-1.0
    }}
  ],
  "density": "high|medium|low"
}}

Rules:
- Only include nodes where you have a clear, specific conclusion
- "discovery" = new finding about how something works
- "decision" = architectural or design choice made with rationale
- "pitfall" = something that didn't work or caused problems
- "technique" = a method or approach that proved effective
- confidence reflects how well-supported the conclusion is by the conversation
- density = how much extractable knowledge this session contains

=== ANALYSIS ===
{analysis}
=== END ===

Output the JSON object only. No markdown fences. /no_think"#;

pub const EVOLVE_PROMPT: &str = r#"You are analyzing knowledge evolution within {project_desc}.

Below is a cluster of related knowledge nodes, ordered chronologically. Each node has an ID (N0, N1...), date, type, and conclusion.

Your task: identify how later nodes relate to earlier ones.

Relationship types:
- **confirms**: later finding validates an earlier conclusion
- **revises**: later finding updates/refines an earlier conclusion
- **supersedes**: later finding completely replaces an earlier conclusion
- **extends**: later finding adds new detail to an earlier conclusion

Output a JSON object:
{{
  "current_understanding": "1-2 sentence summary of the LATEST known-good conclusion",
  "evolution_narrative": "2-3 sentences on how understanding evolved",
  "relationships": [
    {{"from": "N3", "to": "N0", "type": "revises", "detail": "what was revised"}}
  ],
  "final_confidence": 0.0-1.0
}}

=== CLUSTER: {topic} ===
{nodes}
=== END ===

Output ONLY the JSON object. /no_think"#;

pub fn format_pass1(conversation: &str, project_desc: &str, chunk: usize, total: usize) -> String {
    let mut prompt = PASS1_PROMPT
        .replace("{conversation}", conversation)
        .replace("{project_desc}", project_desc);
    if total > 1 {
        let ctx = format!(
            "\n\nNote: This is segment {chunk} of {total} from a long session. \
             Focus on knowledge in THIS segment. Other segments are analyzed separately."
        );
        prompt.push_str(&ctx);
    }
    prompt
}

pub fn format_pass2(analysis: &str) -> String {
    PASS2_PROMPT.replace("{analysis}", analysis)
}

pub fn format_evolve(topic: &str, nodes: &str, project_desc: &str) -> String {
    EVOLVE_PROMPT
        .replace("{topic}", topic)
        .replace("{nodes}", nodes)
        .replace("{project_desc}", project_desc)
}

/// Strip markdown fences, fix trailing commas, extract JSON object
pub fn try_parse_json<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    let trimmed = text.trim();

    // strip ```json ... ```
    let body = if let Some(rest) = trimmed.strip_prefix("```json") {
        rest.strip_suffix("```").unwrap_or(rest).trim()
    } else if let Some(rest) = trimmed.strip_prefix("```") {
        rest.strip_suffix("```").unwrap_or(rest).trim()
    } else {
        trimmed
    };

    // find first { and last }
    let start = body.find('{')?;
    let end = body.rfind('}')?;
    if start > end {
        return None;
    }

    let json_str = &body[start..=end];

    // try direct parse first
    if let Ok(v) = serde_json::from_str(json_str) {
        return Some(v);
    }

    // fix trailing commas: ,\s*} or ,\s*]
    let fixed = regex::Regex::new(r",\s*([}\]])")
        .ok()?
        .replace_all(json_str, "$1");

    serde_json::from_str(&fixed).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn parse_clean_json() {
        let input = r#"{"topic": "TLS", "confidence": 0.9}"#;
        let v: Value = try_parse_json(input).unwrap();
        assert_eq!(v["topic"], "TLS");
    }

    #[test]
    fn parse_with_markdown_fence() {
        let input = "```json\n{\"topic\": \"TLS\"}\n```";
        let v: Value = try_parse_json(input).unwrap();
        assert_eq!(v["topic"], "TLS");
    }

    #[test]
    fn parse_with_bare_fence() {
        let input = "```\n{\"topic\": \"TLS\"}\n```";
        let v: Value = try_parse_json(input).unwrap();
        assert_eq!(v["topic"], "TLS");
    }

    #[test]
    fn parse_trailing_comma() {
        let input = r#"{"nodes": ["a", "b",], "x": 1,}"#;
        let v: Value = try_parse_json(input).unwrap();
        assert_eq!(v["nodes"][0], "a");
        assert_eq!(v["x"], 1);
    }

    #[test]
    fn parse_with_surrounding_text() {
        let input = "Here is the result:\n{\"topic\": \"FFI\"}\nDone.";
        let v: Value = try_parse_json(input).unwrap();
        assert_eq!(v["topic"], "FFI");
    }

    #[test]
    fn parse_no_json_returns_none() {
        let input = "no json here";
        let v: Option<Value> = try_parse_json(input);
        assert!(v.is_none());
    }

    #[test]
    fn parse_nested_with_trailing_comma() {
        let input = r#"```json
{
  "knowledge_nodes": [
    {"topic": "A", "confidence": 0.8,},
    {"topic": "B", "confidence": 0.6},
  ],
  "density": "high",
}
```"#;
        let v: Value = try_parse_json(input).unwrap();
        assert_eq!(v["knowledge_nodes"][1]["topic"], "B");
        assert_eq!(v["density"], "high");
    }
}
