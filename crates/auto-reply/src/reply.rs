use {
    moltis_common::types::{InboundContextV3, ReplyPayload},
    tracing::info,
};

#[cfg(feature = "metrics")]
use moltis_metrics::{auto_reply as auto_reply_metrics, counter, histogram};

/// Main entry point: process an inbound message and produce a reply.
///
/// TODO: load session → parse directives → invoke agent → chunk → return reply
pub async fn get_reply(msg: &InboundContextV3) -> anyhow::Result<ReplyPayload> {
    #[cfg(feature = "metrics")]
    let start = std::time::Instant::now();

    #[cfg(feature = "metrics")]
    counter!(auto_reply_metrics::MESSAGES_RECEIVED_TOTAL).increment(1);

    info!(
        session_id = %msg.session_id,
        session_key = %msg.session_key,
        "incoming message: {}",
        msg.body,
    );

    let result = ReplyPayload {
        text: format!(
            "Echo: {}",
            if msg.body.is_empty() {
                "(no text)"
            } else {
                &msg.body
            }
        ),
        media: None,
        reply_to_message_id: None,
        silent: false,
    };

    #[cfg(feature = "metrics")]
    histogram!(auto_reply_metrics::PROCESSING_DURATION_SECONDS)
        .record(start.elapsed().as_secs_f64());

    Ok(result)
}
