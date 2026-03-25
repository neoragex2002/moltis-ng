# Session Key One-Cut Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Hard-cut the session-key/session-id runtime model so persistence, gateway/web, sandbox, cron, and Telegram all consume the same canonical contracts without fallback or legacy alias paths.

**Architecture:** Start from the owner layer: make persistence and store APIs speak `session_key -> active session_id` plus `session_id -> metadata/state/history`, then move outward to gateway/web and sandbox/cron consumers. Keep Telegram bucket grammar as adapter-local input only, and add focused regression tests for every critical path and reject path.

**Tech Stack:** Rust workspace (`sqlx`, gateway/services/tools/agents crates), Web UI JS assets, markdown issue/design docs, cargo test.

---

### Task 1: Persistence owner hard-cut

**Files:**
- Modify: `crates/sessions/src/lib.rs`
- Modify: `crates/sessions/src/metadata.rs`
- Modify: `crates/sessions/src/state_store.rs`
- Modify: `crates/sessions/src/store.rs`
- Modify: `crates/sessions/migrations/20240205100001_init.sql`
- Modify: `crates/sessions/migrations/20260205120000_session_state.sql`
- Modify: `crates/sessions/migrations/20260205130000_session_branches.sql`

**Step 1: Write failing tests**
- Add persistence tests that require:
  - `active_sessions(session_key -> session_id)`
  - `sessions(session_id, session_key, ...)`
  - `session_state(session_id, ...)`
  - `parent_session_id`
  - `SessionStore(session_id)` round-trip without `:` ↔ `_`

**Step 2: Run targeted tests to verify red**
- Run:
  - `cargo test -p moltis-sessions metadata::`
  - `cargo test -p moltis-sessions state_store::`
  - `cargo test -p moltis-sessions store::`

**Step 3: Implement minimal persistence changes**
- Remove legacy key-as-id assumptions in metadata/state/store.
- Make legacy schema shapes reject directly.

**Step 4: Run targeted tests to verify green**
- Re-run the same `moltis-sessions` tests.

### Task 2: Gateway/runtime and Web contract cut

**Files:**
- Modify: `crates/gateway/src/chat.rs`
- Modify: `crates/gateway/src/session.rs`
- Modify: `crates/gateway/src/services.rs`
- Modify: `crates/gateway/src/methods.rs`
- Modify: `crates/gateway/src/channel_events.rs`
- Modify: `crates/channels/src/plugin.rs`
- Modify: `crates/agents/src/runner.rs`
- Modify: `crates/agents/src/silent_turn.rs`
- Modify: `crates/tools/src/exec.rs`
- Modify: `crates/tools/src/process.rs`
- Modify: `crates/tools/src/spawn_agent.rs`
- Modify: `crates/tools/src/sandbox_packages.rs`
- Modify: `crates/tools/src/session_state.rs`
- Modify: `crates/tools/src/branch_session.rs`
- Modify: `crates/gateway/src/assets/js/app.js`
- Modify: `crates/gateway/src/assets/js/page-chat.js`
- Modify: `crates/gateway/src/assets/js/state.js`
- Modify: `crates/gateway/src/assets/js/stores/session-store.js`
- Modify: `crates/gateway/src/assets/js/onboarding-view.js`
- Modify: `crates/gateway/src/assets/js/sessions.js`
- Modify: `crates/gateway/src/assets/js/components/session-header.js`
- Modify: `crates/gateway/src/assets/js/components/session-list.js`

**Step 1: Write failing tests**
- Add tests for:
  - `sessions.create`
  - `sessions.home`
  - `sessions.resolve` read-only-by-instance behavior
  - no `"main"` fallback
  - no `session:uuid` client generation
  - compaction/silent turn `_sessionId` semantics

**Step 2: Run targeted tests to verify red**
- Run targeted gateway and JS-related Rust tests:
  - `cargo test -p moltis-gateway session::`
  - `cargo test -p moltis-gateway chat::`

**Step 3: Implement minimal runtime/UI changes**
- Make runtime paths instance-based.
- Make Web consume service-owned `displayName`, `sessionKind`, and capability flags.

**Step 4: Run targeted tests to verify green**
- Re-run `moltis-gateway` targeted tests.

### Task 3: Sandbox and cron hard-cut

**Files:**
- Modify: `crates/tools/src/sandbox.rs`
- Modify: `crates/config/src/schema.rs`
- Modify: `crates/config/src/template.rs`
- Modify: `crates/config/src/validate.rs`
- Modify: `crates/gateway/src/server.rs`
- Modify: `crates/gateway/src/chat.rs`

**Step 1: Write failing tests**
- Add tests for:
  - `msb-<readable-slice>-<short-hash>` naming
  - `effectiveSandboxKey` + `containerName` debug exposure
  - reject legacy `container_prefix`
  - `SandboxMode::NonMain` based on canonical `session_key`
  - cron persistent lane uses `system:cron:<bucket_key>` + opaque `session_id`

**Step 2: Run targeted tests to verify red**
- Run:
  - `cargo test -p moltis-tools sandbox::`
  - `cargo test -p moltis-gateway server::`

**Step 3: Implement minimal sandbox/cron changes**
- Remove lossy naming truth.
- Remove configurable runtime prefix.
- Hard-cut cron session semantics.

**Step 4: Run targeted tests to verify green**
- Re-run the same targeted tests.

### Task 4: Final sync and verification

**Files:**
- Modify: `docs/src/refactor/session-key-bucket-key-one-cut.md`
- Modify: `issues/issue-session-key-bucket-key-runtime-and-telegram-one-cut.md`
- Modify: `docs/src/SUMMARY.md`

**Step 1: Sync evidence**
- Update issue/design docs with final file refs and test evidence only.

**Step 2: Run broader verification**
- Run focused package tests first, then broader package suites covering touched crates.

**Step 3: Review and fix**
- Do one more code review pass, fix any issue found, and re-run impacted tests.
