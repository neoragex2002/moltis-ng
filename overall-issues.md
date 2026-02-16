# Overall Issues Audit (2026-02-16)

This document consolidates the issues/risks observed in the current working-tree changes and their nearby integration points. It also proposes a fix order.

## Scope and Method

- Scope: only the current diff and directly related call sites.
- Evidence: file/line references in this repo + a small number of upstream SDK/doc links for protocol truth.
- Verification performed: `lsp_diagnostics` on all modified Rust files (clean).
- Verification note: the initial audit intentionally skipped build/test execution (per request). After fixes landed, targeted tests were run: `cargo test -p moltis-gateway markdown_to_html`, `cargo test -p moltis-config`, `cargo test -p moltis-agents`, plus `node --check crates/gateway/src/assets/js/providers.js`.

### Changed files in this diff (for context)

- `CHANGELOG.md`
- `crates/agents/src/providers/anthropic.rs`
- `crates/agents/src/providers/github_copilot.rs`
- `crates/agents/src/providers/kimi_code.rs`
- `crates/agents/src/providers/mod.rs`
- `crates/agents/src/providers/openai.rs`
- `crates/agents/src/providers/openai_codex.rs`
- `crates/config/src/loader.rs`
- `crates/config/src/template.rs`
- `crates/config/src/validate.rs`
- `crates/gateway/src/assets/js/page-skills.js`
- `crates/gateway/src/assets/js/provider-key-help.js`
- `crates/gateway/src/assets/js/providers.js`
- `crates/gateway/src/auth.rs`
- `crates/gateway/src/lib.rs`
- `crates/gateway/src/provider_setup.rs`
- `crates/gateway/src/server.rs`
- `crates/gateway/src/services.rs`
- `crates/gateway/src/test_support.rs`
- `crates/gateway/src/voice_agent_tools.rs`
- `crates/agents/src/providers/openai_compat.rs`
- `crates/agents/src/providers/openai_responses.rs`
- `crates/agents/src/runner.rs`
- `overall-issues.md`
- `overall-issues.zh.md`

## Current Work Status (post-fix snapshot)

This section reflects the *current working tree* state after implementing the recommended fixes.
The "Detailed Findings" sections below are retained as a historical record of the initial audit (pre-fix).

### Point-by-point Fix Status (clear checklist)

This section explicitly marks what is already fixed in the current working tree.

- [DONE] (P0) `/responses` SSE framing is spec-complete (supports `data:` without a space, multiline `data:` frames, and blank-line boundaries).
  - Evidence: `crates/agents/src/providers/openai_responses.rs:267`.
- [DONE] (P0) Tool-call arguments deltas are correlated correctly for Responses streams (primary key: `(output_index, item_id)`).
  - Evidence: `crates/agents/src/providers/openai_responses.rs:310`.
- [DONE] (P0) `response.function_call_arguments.done` is handled to avoid losing the final payload.
  - Evidence: `crates/agents/src/providers/openai_responses.rs:421`.
- [DONE] (P0) `complete()` fails fast on streamed errors (no “silent success on error”).
  - Evidence: `crates/agents/src/providers/openai_responses.rs:668`.

- [DONE] (P1) `openai-responses` model discovery no longer “disappears” when `/models` fetch fails; it uses OpenAI’s fallback catalog behavior.
  - Evidence: `crates/agents/src/providers/mod.rs:1332` (uses `openai::available_models`), fallback behavior in `crates/agents/src/providers/openai.rs:315`.
- [DONE] (P1) Skills search contract and UI are aligned: `/api/skills/search` returns `drifted`, `eligible`, `missing_bins`, and `install_options` for returned matches.
  - Evidence: `crates/gateway/src/server.rs:4424`.
- [DONE] (P1) Gemini env var docs match runtime (`GEMINI_API_KEY`).
  - Evidence: `crates/config/src/template.rs:127`.

- [DONE] (P2) Kimi base URL normalization prevents accidental `//chat/completions`.
  - Evidence: `crates/agents/src/providers/kimi_code.rs:320`.
- [DONE] (P2) `openai-responses.base_url` semantic constraint (must end with `/v1`) is enforced consistently via config validation/tests.
  - Evidence: `crates/config/src/validate.rs:868` (covered by `cargo test -p moltis-config`).
- [DONE] (P2) Provider alias collisions are no longer silent: duplicate model registrations warn loudly.
  - Evidence: `crates/agents/src/providers/mod.rs:1291`.

- [DONE] (Security) Gateway Markdown rendering blocks raw HTML and unsafe URL schemes for links/images.
  - Evidence: `crates/gateway/src/services.rs:207`, tests at `crates/gateway/src/services.rs:2327`.
- [DONE] (Security) URL scheme filtering strips Unicode whitespace to reduce obfuscated scheme bypasses.
  - Evidence: `crates/gateway/src/services.rs:164`, regression test at `crates/gateway/src/services.rs:2351`.
- [DONE] (Security) Removed unsafe dynamic `innerHTML` concatenation in the providers page; now uses DOM nodes / `textContent`.
  - Evidence: `crates/gateway/src/assets/js/providers.js:1048`.

- [DONE] (Tests) Global config/data dir overrides are serialized in relevant web-ui tests via `TestDirsGuard`.
  - Evidence: `crates/gateway/src/test_support.rs:13`, usage in `crates/gateway/src/server.rs:5556` and `crates/gateway/src/server.rs:6232`.

- [DONE] (JS robustness) Avoided empty catch blocks in skills UI error parsing.
  - Evidence: `crates/gateway/src/assets/js/page-skills.js:133`.

- [TODO] (P3) Retry/backoff improvements (429, jitter, and honoring `Retry-After`).
  - Historical context: `crates/agents/src/runner.rs:45`.

- [TODO] Repo hygiene: working tree is still uncommitted on `main` (needs atomic commits / PR).

### Fixed / Implemented

- **OpenAI Responses `/responses` streaming correctness** is fixed in `crates/agents/src/providers/openai_responses.rs`:
  - Spec-complete SSE framing: accepts both `data:` and `data: `, supports multiline `data:` frames, and uses blank-line boundaries (`crates/agents/src/providers/openai_responses.rs:267`).
  - Tool-call arguments are correlated primarily by `(output_index, item_id)` with safe fallbacks (`crates/agents/src/providers/openai_responses.rs:310`).
  - `response.function_call_arguments.done` is handled to avoid losing the final payload (`crates/agents/src/providers/openai_responses.rs:421`).
  - `complete()` fails fast on streamed `error` events (no “silent success on error”) (`crates/agents/src/providers/openai_responses.rs:668`).

- **Skills search contract vs UI expectations** is aligned:
  - `/api/skills/search` still uses the manifest fast-path, but enriches returned matches with `description`, `drifted`, `eligible`, `missing_bins`, and `install_options` by reading `SKILL.md` only for returned results (`crates/gateway/src/server.rs:4424`).

- **Gemini env var docs** are consistent with runtime (`GEMINI_API_KEY`) (`crates/config/src/template.rs:127`).

- **Kimi base URL normalization** prevents `//chat/completions` (`crates/agents/src/providers/kimi_code.rs:230`).

- **`openai-responses.base_url` semantic validation** (“must end with `/v1`”) is covered by config tests (`crates/config/src/validate.rs` and `cargo test -p moltis-config`).

- **Provider alias collision visibility** improved: duplicate model registrations now warn loudly instead of silently skipping (`crates/agents/src/providers/mod.rs:1291`).

- **Gateway Markdown XSS hardening**:
  - `markdown_to_html` drops raw HTML and blocks unsafe link/image URL schemes (`crates/gateway/src/services.rs:207`).
  - URL filtering strips *Unicode whitespace* (not ASCII-only) to reduce obfuscated-scheme bypass risk (`crates/gateway/src/services.rs:164`).

- **Gateway UI XSS sink removal (providers page)**:
  - Removed dynamic HTML concatenation; now uses DOM nodes / `textContent` (keeps `innerHTML = ""` only for clearing) (`crates/gateway/src/assets/js/providers.js:1070`).

- **Test stability for global dir overrides**:
  - Introduced `TestDirsGuard` and updated the skills-search test to serialize global overrides (`crates/gateway/src/test_support.rs:13`, `crates/gateway/src/server.rs:6232`).

### Verification Evidence (targeted)

- `cargo test -p moltis-gateway markdown_to_html` (includes URL-scheme + Unicode-whitespace regression tests).
- `cargo test -p moltis-config`.
- `cargo test -p moltis-agents`.
- `node --check crates/gateway/src/assets/js/providers.js`.

### Remaining Work

- **Repo hygiene**: changes are currently uncommitted on `main`. Next step is to split into atomic commits (Conventional style is dominant in this repo) and optionally open a PR.
- **Optional follow-ups (not addressed here)**: retry/backoff enhancements (429 + jitter + `Retry-After`), and potential SSE parser max-buffer cap (defense-in-depth).

## Executive Summary (Priority Overview)

P0 (correctness / user-visible breakage risk)

- OpenAI Responses `/responses` streaming: SSE parsing is not spec-complete and tool-call argument deltas are correlated using the wrong key (likely to mis-attribute tool args under parallel/interleaving).
- OpenAI Responses `complete()` can “swallow” stream errors and return a successful but partial/empty response.

P1 (reliability / usability)

- `openai-responses` provider model discovery may register **zero models** if `/models` discovery fails and no models are configured.
- Skills `/api/skills/search` fast-path returns a reduced schema that the UI assumes contains `drifted`/`eligible` semantics; badges and dependency warnings degrade silently.
- Config template docs show the wrong Gemini env var name (`GOOGLE_API_KEY` vs runtime `GEMINI_API_KEY`).

P2 (compatibility / consistency)

- Kimi base URL concatenation can produce `//chat/completions` if `base_url` ends with `/`.
- `openai-responses` base URL “must end with `/v1`” is enforced in the UI flow but not in `moltis config check` validation.
- Provider alias collisions can cause silent model registration skipping across providers.

P3 (quality / operability)

- Retry/backoff policy is very limited (no 429 patterns; fixed delay; no jitter); rate-limit reset information is not propagated end-to-end.
- One new/updated test modifies global config/data dir overrides without using the global guard, risking flakes under parallel tests.

## Detailed Findings

### 1) OpenAI Responses API (`/responses`) provider

Files:

- Primary implementation: `crates/agents/src/providers/openai_responses.rs`
- Shared OpenAI-compatible SSE parsing: `crates/agents/src/providers/openai_compat.rs` (contrast)

#### 1.1 SSE parsing is not spec-complete (frame boundaries, `data:` variants, multiline)

Observed behavior in this repo:

- The parser ignores any line that does not start with `data: ` (requires a literal space): `crates/agents/src/providers/openai_responses.rs:239`.
- The parser consumes the stream line-by-line (`find('\n')`) rather than assembling an SSE message on blank-line boundaries: `crates/agents/src/providers/openai_responses.rs:231`.
- Multiple `data:` lines for one SSE event are not concatenated; JSON parsing would fail or events will be dropped: `crates/agents/src/providers/openai_responses.rs:256`.

Upstream evidence (protocol truth):

- Node SDK SSE decoder supports `data:` with or without a leading space and concatenates multiple `data:` lines with `\n`: `https://raw.githubusercontent.com/openai/openai-node/fe49a7b4826956bf80445f379eee6039a478d410/src/core/streaming.ts`.

Impact:

- Any gateway/SDK-compliant SSE framing that uses `data:` without a space, multiline data, or event-block framing can cause dropped events or premature “stream ended unexpectedly”.

#### 1.2 Tool-call argument delta correlation uses the wrong key

Observed behavior in this repo:

- Tool-call start is keyed by `call_id` read from `response.output_item.added` item: `crates/agents/src/providers/openai_responses.rs:270`–`crates/agents/src/providers/openai_responses.rs:279`.
- Tool-call arguments delta is correlated via top-level `call_id`, and falls back to a guessed index (`current_tool_index - 1`, else `0`): `crates/agents/src/providers/openai_responses.rs:282`–`crates/agents/src/providers/openai_responses.rs:301`.
- The `response.function_call_arguments.done` event is explicitly ignored: `crates/agents/src/providers/openai_responses.rs:304`.

Upstream evidence (Responses streaming schema):

- `response.function_call_arguments.delta` carries `item_id` + `output_index`, not `call_id`: `https://raw.githubusercontent.com/openai/openai-python/3e0c05b84a2056870abf3bd6a5e7849020209cc3/src/openai/types/responses/response_function_call_arguments_delta_event.py`.
- The `call_id` lives on the `function_call` output item itself (not on the delta event): `https://raw.githubusercontent.com/openai/openai-python/3e0c05b84a2056870abf3bd6a5e7849020209cc3/src/openai/types/responses/response_function_tool_call.py`.

Impact:

- Under parallel tool calls and interleaving events, argument deltas can be appended to the wrong tool call, producing invalid arguments and “wrong tool invocation”.
- Ignoring `...arguments.done` can drop the final/coalesced arguments for gateways that send the completed payload primarily in the done event.

#### 1.3 `complete()` can swallow stream errors and return success

Observed behavior in this repo:

- Collector ignores `StreamEvent::Error`: `crates/agents/src/providers/openai_responses.rs:171`–`crates/agents/src/providers/openai_responses.rs:182`.
- In `complete()`, the stream loop treats `StreamEvent::Error(_)` as a terminal “done” and returns `Ok(collector.into_completion())` anyway: `crates/agents/src/providers/openai_responses.rs:518`–`crates/agents/src/providers/openai_responses.rs:525`.

Impact:

- A provider-side failure can manifest as an apparently successful response with missing text and/or incomplete tool calls.

#### 1.4 Potential compatibility risk: forced `OpenAI-Beta` header

Observed behavior in this repo:

- Every `/responses` call sets `OpenAI-Beta: responses=experimental`: `crates/agents/src/providers/openai_responses.rs:155`–`crates/agents/src/providers/openai_responses.rs:160`.

Upstream evidence (current official SDKs do not require it):

- The OpenAI Node/Python SDKs implement `/responses` without injecting a beta header by default; they rely on standard auth + content-type.

Impact:

- Most gateways will ignore unknown headers, but strict proxies or stable endpoints may reject or mishandle this header.

#### 1.5 Non-deterministic `ToolCallComplete` ordering (minor)

- Completion events iterate `HashMap::keys()` without sorting on `[DONE]` and on `response.completed`: `crates/agents/src/providers/openai_responses.rs:244`–`crates/agents/src/providers/openai_responses.rs:246`, `crates/agents/src/providers/openai_responses.rs:317`–`crates/agents/src/providers/openai_responses.rs:319`.

Impact:

- If any downstream consumer assumes stable ordering, behavior can vary run-to-run.

#### 1.6 Note: tests encode the same (likely incorrect) schema assumptions

- The unit test fixtures use `call_id` on `response.function_call_arguments.delta`: `crates/agents/src/providers/openai_responses.rs:655`–`crates/agents/src/providers/openai_responses.rs:656`.

Impact:

- Tests may lock in an incompatible event shape and prevent future corrections unless updated together.

---

### 2) OpenAI `/chat/completions` → `/responses` fallback behavior

File: `crates/agents/src/providers/openai.rs`

#### 2.1 Fallback trigger is string-matching and can be brittle

- Responses-only detection relies on body substring checks: `crates/agents/src/providers/openai.rs:223`–`crates/agents/src/providers/openai.rs:227`.

Impact:

- Gateways that return a different error message shape may not trigger fallback even though `/responses` would work.

#### 2.2 Fallback is restricted to the OpenAI Platform host

- `base_url_is_openai_platform()` checks `api.openai.com` only: `crates/agents/src/providers/openai.rs:250`–`crates/agents/src/providers/openai.rs:256`.

Impact:

- For OpenAI-compatible gateways that do support `/responses`, users may still see unsupported-model failures rather than a successful fallback.

---

### 3) Provider registry / discovery / config & UI consistency

Files:

- Provider registry: `crates/agents/src/providers/mod.rs`
- Config template: `crates/config/src/template.rs`
- Config validator: `crates/config/src/validate.rs`
- Provider setup RPC: `crates/gateway/src/provider_setup.rs`
- Provider UI: `crates/gateway/src/assets/js/providers.js`

#### 3.1 `openai-responses` model discovery can produce zero registered models

- `openai` uses `openai::available_models()` (includes fallback catalog): `crates/agents/src/providers/mod.rs:1273`–`crates/agents/src/providers/mod.rs:1279`.
- `openai-responses` uses `openai::live_models()` and on failure returns `Vec::new()` (no fallback): `crates/agents/src/providers/mod.rs:1318`–`crates/agents/src/providers/mod.rs:1333`.

Impact:

- If discovery fails and config has no `models = [...]`, `openai-responses` effectively disappears from the model list.

#### 3.2 Provider alias collisions can cause silent model skip

- Namespacing is `{provider_label}::{model_id}`: `crates/agents/src/providers/mod.rs:68`–`crates/agents/src/providers/mod.rs:73`.
- `openai` label is `alias.unwrap_or("openai")`: `crates/agents/src/providers/mod.rs:1269`–`crates/agents/src/providers/mod.rs:1271`.
- `openai-responses` label is `alias.unwrap_or("openai-responses")`: `crates/agents/src/providers/mod.rs:1315`–`crates/agents/src/providers/mod.rs:1317`.
- Duplicate `(provider_label, model_id)` registrations are silently skipped via `has_provider_model`: `crates/agents/src/providers/mod.rs:1283`–`crates/agents/src/providers/mod.rs:1285`, `crates/agents/src/providers/mod.rs:1339`–`crates/agents/src/providers/mod.rs:1341`.

Impact:

- If two different providers share the same alias and model IDs overlap, one set will silently not register.

#### 3.3 Gemini env var mismatch: template vs runtime

- Template says Gemini `api_key` can come from `GOOGLE_API_KEY`: `crates/config/src/template.rs:124`–`crates/config/src/template.rs:129`.
- Runtime provider metadata uses `GEMINI_API_KEY`: `crates/gateway/src/provider_setup.rs:507`–`crates/gateway/src/provider_setup.rs:515`.
- Registry also uses `GEMINI_API_KEY` for genai defaults: `crates/agents/src/providers/mod.rs:900`–`crates/agents/src/providers/mod.rs:906`.

Impact:

- Users following the template will set the wrong env var and Gemini will not auto-configure.

#### 3.4 `openai-responses` base_url “must end with `/v1`” is enforced in UI but not in config validation

- UI has explicit hint text: `crates/gateway/src/assets/js/providers.js:41`–`crates/gateway/src/assets/js/providers.js:52`, and the label/hint around `openai-responses` endpoint: `crates/gateway/src/assets/js/providers.js:186`–`crates/gateway/src/assets/js/providers.js:201`.
- Template documents the constraint: `crates/config/src/template.rs:114`–`crates/config/src/template.rs:123`.
- Provider setup enforces the constraint for UI save/validate flows (see helper): `crates/gateway/src/provider_setup.rs:773`–`crates/gateway/src/provider_setup.rs:783`.
- Config validation (`moltis config check`) currently knows `base_url` as a leaf key but has no provider-specific semantic enforcement: `crates/config/src/validate.rs:111`–`crates/config/src/validate.rs:119`.

Impact:

- Misconfigurations can pass `config check` but fail later at runtime.

#### 3.5 Kimi base URL concatenation can produce double slashes

- Requests are built with `format!("{}/chat/completions", self.base_url)` (no trimming): `crates/agents/src/providers/kimi_code.rs:226`–`crates/agents/src/providers/kimi_code.rs:230` and `crates/agents/src/providers/kimi_code.rs:315`–`crates/agents/src/providers/kimi_code.rs:319`.

Impact:

- A `base_url` ending with `/` can generate `...//chat/completions` which some servers reject.

#### 3.6 Kimi endpoint is not editable in the provider modal

- Endpoint input is only shown for `OPENAI_COMPATIBLE_PROVIDERS`, which does not include `kimi-code`: `crates/gateway/src/assets/js/providers.js:41`–`crates/gateway/src/assets/js/providers.js:52`.
- Yet `kimi-code` is present in the backend known providers list with a default base URL: `crates/gateway/src/provider_setup.rs:624`–`crates/gateway/src/provider_setup.rs:632`.

Impact:

- The backend supports a base URL concept for Kimi, but the UI cannot configure it through the modal.

---

### 4) Skills search endpoint contract (`/api/skills/search`) vs UI expectations

Files:

- Server handler and fast-path search: `crates/gateway/src/server.rs`
- UI consumer and badges: `crates/gateway/src/assets/js/page-skills.js`
- Full (slower) enriched list: `crates/gateway/src/services.rs` (`repos_list_full`)

#### 4.1 Fast-path search intentionally returns reduced fields

- Handler uses manifest fast-path for performance: `crates/gateway/src/server.rs:4410`–`crates/gateway/src/server.rs:4413`.
- Response object includes `eligible: true` and `missing_bins: []` unconditionally: `crates/gateway/src/server.rs:4464`–`crates/gateway/src/server.rs:4473`.

Impact:

- UI autocomplete badges degrade silently.

#### 4.2 UI reads fields that search results do not provide

- UI fetches `/api/skills/search`: `crates/gateway/src/assets/js/page-skills.js:133`–`crates/gateway/src/assets/js/page-skills.js:151`.
- Autocomplete shows badges for `drifted` and `eligible === false`: `crates/gateway/src/assets/js/page-skills.js:639`–`crates/gateway/src/assets/js/page-skills.js:640`.
- Missing dependency section depends on `eligible === false` and `missing_bins`: `crates/gateway/src/assets/js/page-skills.js:311`–`crates/gateway/src/assets/js/page-skills.js:316`.

Impact:

- “source changed” / “blocked” badges never show for search results.

#### 4.3 Contrast: full repo list includes drift + eligibility semantics

- `repos_list_full` includes `drifted` and computes eligibility for SKILL.md repos: `crates/gateway/src/services.rs:818`–`crates/gateway/src/services.rs:873`.

Tradeoff:

- Fast-path search is good for latency and avoids scanning large repos.
- However, it currently drops semantic information the UI is designed to show.

---

### 5) Retry/backoff and error UX (cross-cutting)

Files:

- Agent loop runner retry patterns: `crates/agents/src/runner.rs`
- Gateway error parsing: `crates/gateway/src/chat_error.rs`

#### 5.1 Retry policy is narrow and fixed-delay

- Retryable patterns are 5xx-like strings only; there is no explicit `http 429` pattern: `crates/agents/src/runner.rs:45`–`crates/agents/src/runner.rs:56`.
- Retry delay is a constant 2 seconds: `crates/agents/src/runner.rs:65`–`crates/agents/src/runner.rs:67`.

Impact:

- Rate-limits and some transient network errors may not retry (or may retry too aggressively / without jitter).

#### 5.2 Reset/retry metadata is not preserved end-to-end

- `chat_error` extracts `resets_at` from JSON bodies but does not derive it from headers such as `Retry-After`: `crates/gateway/src/chat_error.rs:186`–`crates/gateway/src/chat_error.rs:203`.

Impact:

- The UI cannot reliably show “retry in X seconds” when providers only communicate via headers.

---

### 6) Test stability around global config/data dir overrides

Files:

- Global guard: `crates/gateway/src/test_support.rs`
- Skills search test: `crates/gateway/src/server.rs`

#### 6.1 Guard exists but one test bypasses it

- Guard serializes global overrides and clears on drop: `crates/gateway/src/test_support.rs:5`–`crates/gateway/src/test_support.rs:43`.
- Skills search test sets global data dir directly without the guard: `crates/gateway/src/server.rs:6213` and clears at `crates/gateway/src/server.rs:6252`.

Impact:

- Parallel test execution can race on global config/data dir state, causing flakes.

## Recommended Fix Order (with rationale)

### Phase 1 (P0): Make `/responses` correct and fail-fast

Goal: prevent wrong tool invocations and prevent “silent success on error”.

1) Fix `complete()` error propagation

- Change `complete()` to return an error if a stream yields `StreamEvent::Error(_)`.
- Add a regression test that produces an `error` event and asserts `complete()` returns `Err`.

Evidence: `crates/agents/src/providers/openai_responses.rs:518`–`crates/agents/src/providers/openai_responses.rs:525`.

2) Implement spec-complete SSE parsing for `/responses`

- Accept both `data:` and `data: `.
- Assemble frames using blank-line boundaries; concatenate multi-line `data:` with `\n`.

Evidence: current line-based parsing at `crates/agents/src/providers/openai_responses.rs:231` and strict prefix at `crates/agents/src/providers/openai_responses.rs:239`.

3) Fix tool-call delta correlation to use `item_id` + `output_index`

- Track output items by `(output_index, item_id)`.
- When receiving `response.output_item.added` with a function_call item, bind that output item’s `(output_index, item.id or call_id)` into your internal index mapping.
- For `response.function_call_arguments.delta/done`, map using `(output_index, item_id)` and accumulate arguments.

Upstream schema evidence: `response_function_call_arguments_delta_event.py` (no `call_id`, uses `item_id/output_index`).

4) Update tests to match the correct event schemas

- Replace fixtures that attach `call_id` at the delta event level.

Evidence: `crates/agents/src/providers/openai_responses.rs:655`–`crates/agents/src/providers/openai_responses.rs:656`.

### Phase 2 (P1): Make provider discovery and skills search reliable

Goal: avoid “provider disappears” and restore/align UI semantics.

5) Add fallback catalog for `openai-responses` discovery

- Mirror `openai::available_models()` behavior (fallback catalog) or provide a minimal static catalog for the responses-only provider.

Evidence: `openai` uses fallback `available_models` at `crates/agents/src/providers/mod.rs:1273`–`crates/agents/src/providers/mod.rs:1279`; `openai-responses` does not at `crates/agents/src/providers/mod.rs:1318`–`crates/agents/src/providers/mod.rs:1333`.

6) Decide and enforce a contract for `/api/skills/search`

Option A (preferred): return lightweight `drifted` and real `eligible/missing_bins` for only top-N matches.

- Use manifest/repo-level drift information + run requirements checks only on returned results.

Option B: keep reduced contract, but update the UI to not expect `drifted/eligible/missing_bins` in autocomplete.

Evidence: server response hardcodes `eligible/missing_bins` at `crates/gateway/src/server.rs:4464`–`crates/gateway/src/server.rs:4473`; UI uses those fields in `crates/gateway/src/assets/js/page-skills.js:639`–`crates/gateway/src/assets/js/page-skills.js:640`.

7) Fix Gemini env var doc mismatch

- Update `crates/config/src/template.rs:127` to reference `GEMINI_API_KEY`, or support both env var names consistently.

Evidence: `crates/config/src/template.rs:127` vs `crates/gateway/src/provider_setup.rs:511`.

### Phase 3 (P2): Compatibility and consistency improvements

8) Normalize Kimi base_url on write or on request build

- Trim trailing `/` before appending `/chat/completions`.

Evidence: `crates/agents/src/providers/kimi_code.rs:228` and `crates/agents/src/providers/kimi_code.rs:317`.

9) Add semantic validation for `openai-responses.base_url` ending with `/v1`

- UI already enforces it; add it to config validation so `moltis config check` catches it.

Evidence: UI enforcement helper in `crates/gateway/src/provider_setup.rs:773`–`crates/gateway/src/provider_setup.rs:783`, schema knows `base_url` key at `crates/config/src/validate.rs:111`–`crates/config/src/validate.rs:119`.

10) Define an alias collision policy

- Either reject duplicates across providers, or namespace by provider type (e.g. `openai-responses:<alias>`), or incorporate provider config name into the namespace key.

Evidence: `crates/agents/src/providers/mod.rs:68`–`crates/agents/src/providers/mod.rs:73` and alias usage for both openai providers.

### Phase 4 (P3): Operability and test stability

11) Improve retry/backoff

- Consider retrying 429 with backoff/jitter and honoring `Retry-After` where available.

Evidence: retry patterns in `crates/agents/src/runner.rs:45`–`crates/agents/src/runner.rs:56` and fixed delay at `crates/agents/src/runner.rs:65`–`crates/agents/src/runner.rs:67`.

12) Use `TestDirsGuard` in the skills search test

- Wrap `skills_search_uses_manifest_and_returns_matches` with the guard to serialize global dir overrides.

Evidence: guard in `crates/gateway/src/test_support.rs:5`–`crates/gateway/src/test_support.rs:43`; direct override in `crates/gateway/src/server.rs:6213`.

## Suggested “Done” Criteria per Phase

- Phase 1 done: `/responses` parser passes unit tests that cover `data:` (no-space), multiline `data:` frames, and item_id/output_index correlation; `complete()` returns `Err` on stream errors.
- Phase 2 done: `openai-responses` always registers at least one model when enabled (even if discovery fails); skills search/autocomplete semantics are consistent (either server returns fields or UI stops expecting them); Gemini env var docs match runtime.
- Phase 3 done: Kimi base_url is normalized; config validation catches non-`/v1` endpoints for `openai-responses`; alias collision behavior is defined and enforced.
- Phase 4 done: retry policy handles rate limits and transient errors with backoff; tests involving global dir overrides are race-free.
