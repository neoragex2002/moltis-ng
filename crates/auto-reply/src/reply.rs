use {
    moltis_common::types::{MsgContext, ReplyPayload},
    tracing::info,
};

#[cfg(feature = "metrics")]
use moltis_metrics::{auto_reply as auto_reply_metrics, counter, histogram, labels};

/// Main entry point: process an inbound message and produce a reply.
///
/// TODO: load session → parse directives → invoke agent → chunk → return reply
pub async fn get_reply(msg: &MsgContext) -> anyhow::Result<ReplyPayload> {
    #[cfg(feature = "metrics")]
    let start = std::time::Instant::now();

    #[cfg(feature = "metrics")]
    counter!(
        auto_reply_metrics::MESSAGES_RECEIVED_TOTAL,
        labels::CHANNEL => msg.chan_type.clone()
    )
    .increment(1);

    info!(
        chan_type = %msg.chan_type,
        chan_account_key = %msg.chan_account_key,
        from = %msg.from,
        sender = msg.sender_name.as_deref().unwrap_or("unknown"),
        chat_type = ?msg.chat_type,
        session_id = %msg.session_id,
        chan_chat_key = %msg.chan_chat_key,
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
        reply_to_message_id: msg.reply_to_message_id.clone(),
        silent: false,
    };

    #[cfg(feature = "metrics")]
    histogram!(
        auto_reply_metrics::PROCESSING_DURATION_SECONDS,
        labels::CHANNEL => msg.chan_type.clone()
    )
    .record(start.elapsed().as_secs_f64());

    Ok(result)
}
