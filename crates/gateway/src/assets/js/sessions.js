// ── Sessions: list, switch, status helpers ──────────────────

import {
	appendChannelFooter,
	chatAddMsg,
	chatAddMsgWithImages,
	highlightAndScroll,
	removeThinking,
	scrollChatToBottom,
	stripChannelPrefix,
	updateTokenBar,
} from "./chat-ui.js";
import * as gon from "./gon.js";
import {
	formatTokens,
	renderAudioPlayer,
	renderMarkdown,
	renderScreenshot,
	sendRpc,
	toolCallSummary,
} from "./helpers.js";
import { updateSessionProjectSelect } from "./project-combo.js";
import { currentPrefix, navigate, sessionPath } from "./router.js";
import { updateSandboxImageUI, updateSandboxUI } from "./sandbox.js";
import * as S from "./state.js";
import { modelStore } from "./stores/model-store.js";
import { projectStore } from "./stores/project-store.js";
import { sessionStore } from "./stores/session-store.js";
import { confirmDialog } from "./ui.js";

var SESSION_PREVIEW_MAX_CHARS = 200;
var switchGenerationCounter = 0;

function activeSessionId() {
	return sessionStore.activeSessionId.value || "";
}

function truncateSessionPreview(text) {
	var trimmed = (text || "").trim();
	if (!trimmed) return "";
	var chars = Array.from(trimmed);
	if (chars.length <= SESSION_PREVIEW_MAX_CHARS) return trimmed;
	return `${chars.slice(0, SESSION_PREVIEW_MAX_CHARS).join("")}…`;
}

// ── Fetch & render ──────────────────────────────────────────

export function fetchSessions() {
	sendRpc("sessions.list", {}).then((res) => {
		if (!res?.ok) return;
		var incoming = res.payload || [];
		sessionStore.setAll(incoming);
		renderSessionList();
	});
}

/** Clear history for the currently active session and reset local UI state. */
export function clearActiveSession() {
	var sessionId = activeSessionId();
	if (!sessionId) {
		return Promise.resolve({ ok: false, error: { message: "No active session" } });
	}
	var session = sessionStore.getById(sessionId);
	var prevHistoryIdx = session?.lastHistoryIndex.value ?? -1;
	var prevSeq = session?.chatSeq.value ?? 0;
	if (session) {
		session.lastHistoryIndex.value = -1;
		session.chatSeq.value = 0;
		session.sessionTokens.value = { input: 0, output: 0 };
	}
	return sendRpc("chat.clear", { _sessionId: sessionId }).then((res) => {
		if (res?.ok) {
			if (S.chatMsgBox) S.chatMsgBox.textContent = "";
			updateTokenBar();
			if (session) session.syncCounts(0, 0);
			fetchSessions();
			return res;
		}
		if (session) {
			session.lastHistoryIndex.value = prevHistoryIdx;
			session.chatSeq.value = prevSeq;
		}
		chatAddMsg("error", res?.error?.message || "Clear failed");
		return res;
	});
}

/** Re-fetch the active session entry and restore sandbox/model state. */
export function refreshActiveSession() {
	var sessionId = activeSessionId();
	if (!sessionId) return;
	sendRpc("sessions.resolve", { sessionId: sessionId }).then((res) => {
		if (!(res?.ok && res.payload)) return;
		var entry = res.payload.entry || res.payload;
		restoreSessionState(entry, entry.projectId);
	});
}

function homeSessionId() {
	return sessionStore.defaultSessionId();
}

function ensureHomeSession() {
	return sendRpc("sessions.home", {}).then((res) => {
		if (!(res?.ok && res.payload?.sessionId)) return null;
		sessionStore.upsert(res.payload);
		return res.payload.sessionId;
	});
}

function isMissingSessionSwitchError(res) {
	var message = res?.error?.message || "";
	return message.includes("session resolve failed:") && message.includes("not found");
}

// ── Session list ─────────────────────────────────────────────
// The Preact SessionList component is mounted once from app.js and
// auto-rerenders from signals.  This function handles the imperative
// Clear button visibility that lives outside the component.

export function renderSessionList() {
	updateClearAllVisibility();
}

// ── Status helpers ──────────────────────────────────────────

export function setSessionReplying(sessionId, replying) {
	var session = sessionStore.getById(sessionId);
	if (session) session.replying.value = replying;
}

export function setSessionUnread(sessionId, unread) {
	var session = sessionStore.getById(sessionId);
	if (session) session.localUnread.value = unread;
}

export function bumpSessionCount(sessionId, increment) {
	var session = sessionStore.getById(sessionId);
	if (session) session.bumpCount(increment);
}

/** Set first-message preview optimistically so sidebar updates without reload. */
export function seedSessionPreviewFromUserText(sessionId, text) {
	var preview = truncateSessionPreview(text);
	if (!preview) return;
	var now = Date.now();

	var session = sessionStore.getById(sessionId);
	if (session && !session.preview) {
		session.preview = preview;
		session.updatedAt = now;
		session.dataVersion.value++;
	}
}

// ── New session button ──────────────────────────────────────
var newSessionBtn = S.$("newSessionBtn");
newSessionBtn.addEventListener("click", () => {
	var filterId = projectStore.projectFilterId.value;
	var params = {};
	if (filterId) params.projectId = filterId;
	sendRpc("sessions.create", params).then((res) => {
		if (!(res?.ok && res.payload?.sessionId)) return;
		var sessionId = res.payload.sessionId;
		sessionStore.upsert(res.payload);
		if (currentPrefix === "/chats") {
			switchSession(sessionId, null, filterId || undefined);
		} else {
			navigate(sessionPath(sessionId));
		}
	});
});

// ── Clear all sessions button ───────────────────────────────
var clearAllBtn = S.$("clearAllSessionsBtn");

/** Show the Clear button only when there are deletable agent sessions. */
function updateClearAllVisibility() {
	if (!clearAllBtn) return;
	var allSessions = sessionStore.sessions.value;
	var hasClearable = allSessions.some((s) => s.canDelete && s.sessionKind === "agent");
	clearAllBtn.classList.toggle("hidden", !hasClearable);
}

if (clearAllBtn) {
	clearAllBtn.addEventListener("click", () => {
		var allSessions = sessionStore.sessions.value;
		var count = allSessions.filter((s) => s.canDelete && s.sessionKind === "agent").length;
		if (count === 0) return;
		confirmDialog(`Delete ${count} session${count !== 1 ? "s" : ""}?`).then((yes) => {
			if (!yes) return;
			clearAllBtn.disabled = true;
			clearAllBtn.textContent = "Clearing\u2026";
			sendRpc("sessions.clear_all", {}).then((res) => {
				clearAllBtn.disabled = false;
				clearAllBtn.textContent = "Clear All";
				if (res?.ok) {
					var active = sessionStore.getById(sessionStore.activeSessionId.value);
					var wasKept = !active || !(active.canDelete && active.sessionKind === "agent");
					if (!wasKept) {
						ensureHomeSession().then((sessionId) => {
							if (sessionId) switchSession(sessionId);
						});
					}
					fetchSessions();
				}
			});
		});
	});
}

// ── Re-render session list on project filter change ─────────
document.addEventListener("moltis:render-session-list", renderSessionList);

// ── MCP toggle restore ──────────────────────────────────────
function restoreMcpToggle(mcpEnabled) {
	var mcpBtn = S.$("mcpToggleBtn");
	var mcpLabel = S.$("mcpToggleLabel");
	if (mcpBtn) {
		mcpBtn.style.color = mcpEnabled ? "var(--ok)" : "var(--muted)";
		mcpBtn.style.borderColor = mcpEnabled ? "var(--ok)" : "var(--border)";
	}
	if (mcpLabel) mcpLabel.textContent = mcpEnabled ? "MCP" : "MCP off";
}

// ── Switch session ──────────────────────────────────────────

function restoreSessionState(entry, projectId) {
	var effectiveProjectId = entry.projectId || projectId || "";
	projectStore.setActiveProjectId(effectiveProjectId);
	// Dual-write to state.js for backward compat
	S.setActiveProjectId(effectiveProjectId);
	localStorage.setItem("moltis-project", effectiveProjectId);
	updateSessionProjectSelect(effectiveProjectId);
	if (entry.model) {
		modelStore.select(entry.model);
		// Dual-write to state.js for backward compat
		S.setSelectedModelId(entry.model);
		localStorage.setItem("moltis-model", entry.model);
		var found = modelStore.getById(entry.model);
		if (S.modelComboLabel) S.modelComboLabel.textContent = found ? found.displayName || found.id : entry.model;
	}
	updateSandboxUI(entry.sandboxEnabled !== false);
	updateSandboxImageUI(entry.sandboxImage || null);
	restoreMcpToggle(!entry.mcpDisabled);
}

/** Extract text and images from a multimodal content array. */
function parseMultimodalContent(blocks) {
	var text = "";
	var images = [];
	for (var block of blocks) {
		if (block.type === "text") {
			text = block.text || "";
		} else if (block.type === "image_url" && block.image_url?.url) {
			images.push({ dataUrl: block.image_url.url, name: "image" });
		}
	}
	return { text: text, images: images };
}

function renderHistoryUserMessage(msg) {
	var el;
	if (Array.isArray(msg.content)) {
		var parsed = parseMultimodalContent(msg.content);
		var text = msg.channel ? stripChannelPrefix(parsed.text) : parsed.text;
		el = chatAddMsgWithImages("user", text ? renderMarkdown(text) : "", parsed.images);
	} else {
		var userContent = msg.channel ? stripChannelPrefix(msg.content || "") : msg.content || "";
		el = chatAddMsg("user", renderMarkdown(userContent), true);
	}
	if (el && msg.channel) appendChannelFooter(el, msg.channel);
	return el;
}

function createModelFooter(msg) {
	var ft = document.createElement("div");
	ft.className = "msg-model-footer";
	var ftText = msg.provider ? `${msg.provider} / ${msg.model}` : msg.model;
	if (msg.inputTokens || msg.outputTokens) {
		ftText += ` \u00b7 ${formatTokens(msg.inputTokens || 0)} in / ${formatTokens(msg.outputTokens || 0)} out`;
	}
	ft.textContent = ftText;
	return ft;
}

function renderHistoryAssistantMessage(msg) {
	var el;
	if (msg.audio) {
		// Voice response: render audio player first, then transcript text below.
		el = chatAddMsg("assistant", "", true);
	if (el) {
		var filename = msg.audio.split("/").pop();
		var audioSrc = `/api/sessions/${encodeURIComponent(activeSessionId())}/media/${encodeURIComponent(filename)}`;
		renderAudioPlayer(el, audioSrc);
		if (msg.content) {
				var textWrap = document.createElement("div");
				textWrap.className = "mt-2";
				// Safe: renderMarkdown calls esc() first — all user input is HTML-escaped.
				textWrap.innerHTML = renderMarkdown(msg.content); // eslint-disable-line no-unsanitized/property
				el.appendChild(textWrap);
			}
		}
	} else {
		el = chatAddMsg("assistant", renderMarkdown(msg.content || ""), true);
	}
	if (el && msg.model) {
		el.appendChild(createModelFooter(msg));
	}
	if (msg.inputTokens || msg.outputTokens) {
		var session = sessionStore.activeSession.value;
		if (session) {
			session.sessionTokens.value = {
				input: session.sessionTokens.value.input + (msg.inputTokens || 0),
				output: session.sessionTokens.value.output + (msg.outputTokens || 0),
			};
		}
	}
	return el;
}

// biome-ignore lint/complexity/noExcessiveCognitiveComplexity: Sequential result field rendering
function renderHistoryToolResult(msg) {
	var tpl = document.getElementById("tpl-exec-card");
	var frag = tpl.content.cloneNode(true);
	var card = frag.firstElementChild;

	// Remove the "running…" status element — this is a completed result.
	var statusEl = card.querySelector(".exec-status");
	if (statusEl) statusEl.remove();

	// Set command summary from arguments.
	var cmd = toolCallSummary(msg.tool_name, msg.arguments);
	card.querySelector("[data-cmd]").textContent = ` ${cmd}`;

	// Set success/error CSS class (replace the default "running" class).
	card.className = `msg exec-card ${msg.success ? "exec-ok" : "exec-err"}`;

	// Append result output if present.
	if (msg.result) {
		var out = (msg.result.stdout || "").replace(/\n+$/, "");
		if (out) {
			var outEl = document.createElement("pre");
			outEl.className = "exec-output";
			outEl.textContent = out;
			card.appendChild(outEl);
		}
		var stderrText = (msg.result.stderr || "").replace(/\n+$/, "");
		if (stderrText) {
			var errEl = document.createElement("pre");
			errEl.className = "exec-output exec-stderr";
			errEl.textContent = stderrText;
			card.appendChild(errEl);
		}
		if (msg.result.exit_code !== undefined && msg.result.exit_code !== 0) {
			var codeEl = document.createElement("div");
			codeEl.className = "exec-exit";
			codeEl.textContent = `exit ${msg.result.exit_code}`;
			card.appendChild(codeEl);
		}
		// Render persisted screenshot from the media API.
		if (msg.result.screenshot && !msg.result.screenshot.startsWith("data:")) {
			var filename = msg.result.screenshot.split("/").pop();
			var sessionId = activeSessionId() || homeSessionId();
			var mediaSrc = `/api/sessions/${encodeURIComponent(sessionId)}/media/${encodeURIComponent(filename)}`;
			renderScreenshot(card, mediaSrc);
		}
	}

	// Append error detail if present.
	if (!msg.success && msg.error) {
		var errMsg = document.createElement("div");
		errMsg.className = "exec-error-detail";
		errMsg.textContent = msg.error;
		card.appendChild(errMsg);
	}

	if (S.chatMsgBox) S.chatMsgBox.appendChild(card);
	return card;
}

export function appendLastMessageTimestamp(epochMs) {
	if (!S.chatMsgBox) return;
	// Remove any previous last-message timestamp
	var old = S.chatMsgBox.querySelector(".msg-footer-time");
	if (old) old.remove();
	var lastMsg = S.chatMsgBox.lastElementChild;
	if (!lastMsg || lastMsg.classList.contains("user")) return;
	var footer = lastMsg.querySelector(".msg-model-footer");
	if (!footer) {
		footer = document.createElement("div");
		footer.className = "msg-model-footer";
		lastMsg.appendChild(footer);
	}
	var sep = document.createTextNode(" \u00b7 ");
	sep.className = "msg-footer-time";
	var t = document.createElement("time");
	t.className = "msg-footer-time";
	t.setAttribute("data-epoch-ms", String(epochMs));
	t.textContent = new Date(epochMs).toISOString();
	// Wrap separator + time in a span so we can remove both easily
	var wrap = document.createElement("span");
	wrap.className = "msg-footer-time";
	wrap.appendChild(document.createTextNode(" \u00b7 "));
	wrap.appendChild(t);
	footer.appendChild(wrap);
}

function makeThinkingDots() {
	var tpl = document.getElementById("tpl-thinking-dots");
	return tpl.content.cloneNode(true).firstElementChild;
}

function postHistoryLoadActions(sessionId, searchContext, msgEls) {
	sendRpc("chat.context", {
		_sessionId: sessionId,
		draftText: S.chatInput ? S.chatInput.value : "",
	}).then((ctxRes) => {
		var session = sessionStore.getById(sessionId);
		if (session && ctxRes?.ok && ctxRes.payload) {
			var next = ctxRes.payload.tokenDebug ? ctxRes.payload.tokenDebug.nextRequest : null;
			session.contextWindow.value = next && next.contextWindow !== undefined ? next.contextWindow || 0 : 0;
			session.sessionBudget.value = next || null;
			session.toolsEnabled.value = ctxRes.payload.supportsTools !== false;
		}
		updateTokenBar();
	});
	updateTokenBar();

	if (searchContext?.query && S.chatMsgBox) {
		highlightAndScroll(msgEls, searchContext.messageIndex, searchContext.query);
	} else {
		scrollChatToBottom();
	}

	var session = sessionStore.getById(sessionId);
	if (session?.replying.value && S.chatMsgBox) {
		removeThinking();
		var thinkEl = document.createElement("div");
		thinkEl.className = "msg assistant thinking";
		thinkEl.id = "thinkingIndicator";
		thinkEl.appendChild(makeThinkingDots());
		S.chatMsgBox.appendChild(thinkEl);
		scrollChatToBottom();
	}
}

function showWelcomeCard() {
	if (!S.chatMsgBox) return;

	if (modelStore.models.value.length === 0) {
		var noProvTpl = document.getElementById("tpl-no-providers-card");
		if (!noProvTpl) return;
		var noProvCard = noProvTpl.content.cloneNode(true).firstElementChild;
		S.chatMsgBox.appendChild(noProvCard);
		return;
	}

	var tpl = document.getElementById("tpl-welcome-card");
	if (!tpl) return;
	var card = tpl.content.cloneNode(true).firstElementChild;
	var identity = gon.get("identity");
	var userName = identity?.user_name;
	var botName = identity?.name || "moltis";
	var botEmoji = identity?.emoji || "";

	var greetingEl = card.querySelector("[data-welcome-greeting]");
	if (greetingEl) greetingEl.textContent = userName ? `Hello, ${userName}!` : "Hello!";
	var emojiEl = card.querySelector("[data-welcome-emoji]");
	if (emojiEl) emojiEl.textContent = botEmoji;
	var nameEl = card.querySelector("[data-welcome-bot-name]");
	if (nameEl) nameEl.textContent = botName;

	S.chatMsgBox.appendChild(card);
}

export function refreshWelcomeCardIfNeeded() {
	if (!S.chatMsgBox) return;
	var welcomeCard = S.chatMsgBox.querySelector("#welcomeCard");
	var noProvCard = S.chatMsgBox.querySelector("#noProvidersCard");
	var hasModels = modelStore.models.value.length > 0;

	// Wrong variant showing — swap it
	if (hasModels && noProvCard) {
		noProvCard.remove();
		showWelcomeCard();
	} else if (!hasModels && welcomeCard) {
		welcomeCard.remove();
		showWelcomeCard();
	}
}

function ensureSessionInClientStore(sessionId, entry, projectId) {
	var created = { ...entry, sessionId: sessionId };
	if (projectId && !created.projectId) created.projectId = projectId;
	var existing = sessionStore.getById(sessionId);
	if (existing) {
		// Do not clobber an already-hydrated session with a clientOnly placeholder.
		if (created.clientOnly) return existing;
		existing.update(created);
		return existing;
	}
	return sessionStore.upsert(created);
}

export function switchSession(sessionId, searchContext, projectId, options = {}) {
	if (!sessionId) {
		ensureHomeSession().then((homeId) => {
			if (homeId) switchSession(homeId, searchContext, projectId, options);
		});
		return;
	}
	var switchGeneration = ++switchGenerationCounter;
	sessionStore.switchGeneration.value = switchGeneration;
	sessionStore.setActive(sessionId);
	history.replaceState(null, "", sessionPath(sessionId));
	if (S.chatMsgBox) S.chatMsgBox.textContent = "";
	var tray = document.getElementById("queuedMessages");
	if (tray) {
		while (tray.firstChild) tray.removeChild(tray.firstChild);
		tray.classList.add("hidden");
	}
	S.setStreamEl(null);
	var pendingSession = ensureSessionInClientStore(
		sessionId,
		{ sessionId: sessionId, clientOnly: true },
		projectId,
	);
	if (pendingSession) pendingSession.resetViewState();
	updateTokenBar();
	// Preact SessionList auto-rerenders active/unread from signals.

	sessionStore.switchInProgress.value = true;
	var switchParams = { sessionId: sessionId };
	if (projectId) switchParams.projectId = projectId;
	// biome-ignore lint/complexity/noExcessiveCognitiveComplexity: Session switch handles many state updates
	sendRpc("sessions.switch", switchParams).then((res) => {
		if (switchGeneration !== switchGenerationCounter || sessionStore.activeSessionId.value !== sessionId) {
			var activeSessionId = sessionStore.activeSessionId.value || "";
			console.info(
				'[moltis] event="session.switch" reason_code="stale_switch_response" decision="drop" policy="web_ui_session_owner_v1"',
				{
					requested_session_id: sessionId,
					active_session_id: activeSessionId,
					switch_generation: switchGeneration,
				},
			);
			return;
		}
		if (res?.ok && res.payload) {
			var entry = res.payload.entry || {};
			ensureSessionInClientStore(sessionId, entry, projectId);
			restoreSessionState(entry, projectId);
			var history = res.payload.history || [];
			var msgEls = [];
			var sessionEntry = sessionStore.getById(sessionId);
			if (sessionEntry) sessionEntry.sessionTokens.value = { input: 0, output: 0 };
			S.setChatBatchLoading(true);
			history.forEach((msg) => {
				if (msg.role === "user") {
					msgEls.push(renderHistoryUserMessage(msg));
				} else if (msg.role === "assistant") {
					msgEls.push(renderHistoryAssistantMessage(msg));
				} else if (msg.role === "tool_result") {
					msgEls.push(renderHistoryToolResult(msg));
				} else {
					msgEls.push(null);
				}
			});
			S.setChatBatchLoading(false);
			// Resume chatSeq from the highest user message seq in history
			// so the counter continues from where it left off after reload.
			var maxSeq = 0;
			for (var hm of history) {
				if (hm.role === "user" && hm.seq > maxSeq) {
					maxSeq = hm.seq;
				}
			}
			if (history.length === 0) {
				showWelcomeCard();
			}
			if (history.length > 0) {
				var lastMsg = history[history.length - 1];
				var ts = lastMsg.created_at;
				if (ts) appendLastMessageTimestamp(ts);
			}
			// Sync the store entry — syncCounts calls updateBadge() for re-render.
			if (sessionEntry) {
				sessionEntry.syncCounts(history.length, history.length);
				sessionEntry.localUnread.value = false;
				sessionEntry.lastHistoryIndex.value = history.length > 0 ? history.length - 1 : -1;
				sessionEntry.chatSeq.value = maxSeq;
			}
			sessionStore.switchInProgress.value = false;
			postHistoryLoadActions(sessionId, searchContext, msgEls);
			if (S.chatInput) S.chatInput.focus();
			} else {
				sessionStore.switchInProgress.value = false;
				if (isMissingSessionSwitchError(res)) {
					if (options?.source === "restore") {
						console.warn(
							'[moltis] event="session.restore" reason_code="stored_session_missing" decision="fallback_home" policy="web_ui_session_owner_v1"',
							{ stored_session_id: sessionId, active_session_id: sessionId },
						);
					}
					sessionStore.setActive("");
					ensureHomeSession().then((homeId) => {
						if (homeId) switchSession(homeId, searchContext, projectId, options);
					});
				}
				if (S.chatInput) S.chatInput.focus();
			}
	});
}
