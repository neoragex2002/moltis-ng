//! Inbound message processing pipeline — the glue between channels and agents.
//!
//! Flow: channel message → normalize V3 inbound context → resolve route → load session →
//! apply media understanding → parse directives → invoke agent → chunk response →
//! deliver via channel outbound.

pub mod chunk;
pub mod directives;
pub mod queue;
pub mod reply;
