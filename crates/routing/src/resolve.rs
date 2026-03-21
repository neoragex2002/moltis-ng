use moltis_common::types::InboundContextV3;

/// Resolved route: which agent handles this message and which session bucket it belongs to.
#[derive(Debug, Clone)]
pub struct ResolvedRoute {
    pub agent_id: String,
    pub session_key: String,
    pub session_id: Option<String>,
}

/// Resolve which agent should handle a message, following the binding cascade.
pub fn resolve_agent_route(
    msg: &InboundContextV3,
    _config: &serde_json::Value,
) -> anyhow::Result<ResolvedRoute> {
    Ok(ResolvedRoute {
        agent_id: "default".to_string(),
        session_key: msg.session_key.clone(),
        session_id: Some(msg.session_id.clone()),
    })
}
