// ── Session store (signal-based) ─────────────────────────────
//
// Single source of truth for session data. Each session becomes a
// Session class instance with per-session signals for client-side state.

	import { computed, signal } from "@preact/signals";
	import { INVALID_SESSION_LABEL, PENDING_SESSION_LABEL, sessionLabelText } from "../session-label.js";

	// ── Session class ────────────────────────────────────────────

	var missingDisplayNameWarned = new Set();
	function warnMissingDisplayName(sessionId) {
		if (!sessionId) return;
		var key = `session_store_normalize:${sessionId}`;
		if (missingDisplayNameWarned.has(key)) return;
		missingDisplayNameWarned.add(key);
		console.warn(
			'[moltis] event="session.contract_violation" reason_code="missing_display_name" decision="warn" policy="web_ui_session_owner_v1"',
			{ session_id: sessionId, surface: "session_store_normalize" },
		);
	}

	export class Session {
		constructor(serverData) {
			// Server fields (plain properties, set on construction/update)
			this.sessionId = serverData.sessionId;
			this.sessionKey = serverData.sessionKey || "";
			this.label = serverData.label || "";
			this.clientOnly = !!serverData.clientOnly;
			this.displayName = typeof serverData.displayName === "string" ? serverData.displayName.trim() : "";
			if (!this.clientOnly && !this.displayName) warnMissingDisplayName(this.sessionId);
			this.sessionKind = serverData.sessionKind || "";
			this.canRename = !!serverData.canRename;
			this.canDelete = !!serverData.canDelete;
			this.canFork = !!serverData.canFork;
		this.canClear = !!serverData.canClear;
		this.model = serverData.model || "";
		this.provider = serverData.provider || "";
		this.projectId = serverData.projectId || "";
		this.messageCount = serverData.messageCount || 0;
		this.lastSeenMessageCount = serverData.lastSeenMessageCount || 0;
		this.preview = serverData.preview || "";
		this.updatedAt = serverData.updatedAt || 0;
		this.createdAt = serverData.createdAt || 0;
		this.worktreeBranch = serverData.worktreeBranch || "";
		this.channel = serverData.channel || null;
		this.parentSessionId = serverData.parentSessionId || "";
		this.forkPoint = serverData.forkPoint != null ? serverData.forkPoint : null;
		this.mcpDisabled = serverData.mcpDisabled;
		this.archived = serverData.archived;
		this.activeChannel = serverData.activeChannel;
		this.version = serverData.version || 0;

		// Client signals (reactive, per-session)
		this.replying = signal(false);
		this.localUnread = signal(false);
		this.streamText = signal("");
		this.voicePending = signal(false);
		this.lastHistoryIndex = signal(-1);
		this.chatSeq = signal(0);
		this.sessionTokens = signal({ input: 0, output: 0 });
		this.contextWindow = signal(0);
		this.sessionBudget = signal(null);
		this.toolsEnabled = signal(true);
		this.lastToolOutput = signal("");
		// Total message count — reactive signal that drives the sidebar badge.
		// Components read this to show/hide badge and compute unread tinting.
		this.badgeCount = signal(this.messageCount);
		// Bumped whenever plain properties change so subscribed components re-render.
		this.dataVersion = signal(0);
	}

	/** Recalculate badge from current messageCount. */
	updateBadge() {
		this.badgeCount.value = this.messageCount;
	}

		/** Merge server fields, preserving client signals. Returns false if stale. */
		update(serverData) {
			var incoming = serverData.version || 0;
			if (incoming > 0 && this.version > 0 && incoming < this.version) return false;
			this.version = incoming || this.version;
			this.sessionKey = serverData.sessionKey || "";
			this.label = serverData.label || "";
			this.clientOnly = !!serverData.clientOnly;
			this.displayName = typeof serverData.displayName === "string" ? serverData.displayName.trim() : "";
			if (!this.clientOnly && !this.displayName) warnMissingDisplayName(this.sessionId);
			this.sessionKind = serverData.sessionKind || "";
			this.canRename = !!serverData.canRename;
			this.canDelete = !!serverData.canDelete;
			this.canFork = !!serverData.canFork;
		this.canClear = !!serverData.canClear;
		this.model = serverData.model || "";
		this.provider = serverData.provider || "";
		this.projectId = serverData.projectId || "";
		// Only accept server counts when they've caught up with optimistic
		// client bumps. Authoritative resets (/clear, switchSession) use
		// syncCounts() which sets messageCount directly before any fetch.
		var serverCount = serverData.messageCount || 0;
		if (serverCount >= this.messageCount) {
			this.messageCount = serverCount;
			this.lastSeenMessageCount = serverData.lastSeenMessageCount || 0;
			this.preview = serverData.preview || "";
			this.updatedAt = serverData.updatedAt || 0;
		}
		this.createdAt = serverData.createdAt || 0;
		this.worktreeBranch = serverData.worktreeBranch || "";
		this.channel = serverData.channel || null;
		this.parentSessionId = serverData.parentSessionId || "";
		this.forkPoint = serverData.forkPoint != null ? serverData.forkPoint : null;
		this.mcpDisabled = serverData.mcpDisabled;
		this.archived = serverData.archived;
		this.activeChannel = serverData.activeChannel;
		this.updateBadge();
		this.dataVersion.value++;
		return true;
	}

	/** Optimistic bump: increment total and mark seen if active. */
	bumpCount(increment) {
		this.messageCount = (this.messageCount || 0) + increment;
		if (this.sessionId === activeSessionId.value) {
			this.lastSeenMessageCount = this.messageCount;
		}
		this.updateBadge();
	}

	/** Authoritative set (switchSession history, /clear). */
	syncCounts(messageCount, lastSeenMessageCount) {
		this.messageCount = messageCount;
		this.lastSeenMessageCount = lastSeenMessageCount;
		this.updateBadge();
	}

	/** Clear streaming state for this session. */
	resetStreamState() {
		this.streamText.value = "";
		this.voicePending.value = false;
		this.lastToolOutput.value = "";
	}

	resetViewState() {
		this.resetStreamState();
		this.lastHistoryIndex.value = -1;
		this.chatSeq.value = 0;
		this.sessionTokens.value = { input: 0, output: 0 };
		this.contextWindow.value = 0;
		this.sessionBudget.value = null;
		this.toolsEnabled.value = true;
	}
}

	// ── Store signals ────────────────────────────────────────────
	export var sessions = signal([]);
	export var activeSessionId = signal(localStorage.getItem("moltis-sessionId") || "");
	export var switchInProgress = signal(false);
	export var switchGeneration = signal(0);

export var activeSession = computed(() => {
	var sessionId = activeSessionId.value;
	return sessions.value.find((s) => s.sessionId === sessionId) || null;
});

// ── Methods ──────────────────────────────────────────────────

/**
 * Replace the full sessions list from server data.
 * Reuses existing Session instances (matched by key) so their
 * client-side signals (replying, localUnread, streamText) are preserved.
 * New keys get fresh instances. Missing keys are dropped.
 */
export function setAll(serverSessions) {
	var existing = {};
	for (var s of sessions.value) {
		existing[s.sessionId] = s;
	}

	var result = [];
	for (var data of serverSessions) {
		var prev = existing[data.sessionId];
		if (prev) {
			prev.update(data);
			// Preserve client-side flags from old patched objects
			if (data._localUnread) prev.localUnread.value = true;
			if (data._replying) prev.replying.value = true;
			result.push(prev);
		} else {
			var session = new Session(data);
			if (data._localUnread) session.localUnread.value = true;
			if (data._replying) session.replying.value = true;
			result.push(session);
		}
	}

	sessions.value = result;
}

/**
 * Upsert a single session from server data.
 * Reuses existing instance when present; creates and appends when missing.
 */
export function upsert(serverData) {
	if (!(serverData && serverData.sessionId)) return null;
	var prev = getById(serverData.sessionId);
	if (prev) {
		prev.update(serverData);
		return prev;
	}
	var next = new Session(serverData);
	sessions.value = [...sessions.value, next];
	return next;
}

/** Fetch sessions from the server via RPC. */
export function fetch() {
	return import("../helpers.js").then((helpers) => {
		return helpers.sendRpc("sessions.list", {}).then((res) => {
			if (!res?.ok) return;
			setAll(res.payload || []);
		});
	});
}

/** Notify Preact that session data changed (triggers re-render). */
export function notify() {
	sessions.value = [...sessions.value];
}

export function getById(sessionId) {
	return sessions.value.find((s) => s.sessionId === sessionId) || null;
}

export function setActive(sessionId) {
	activeSessionId.value = sessionId;
	if (sessionId) {
		localStorage.setItem("moltis-sessionId", sessionId);
	} else {
		localStorage.removeItem("moltis-sessionId");
	}
}

export function defaultSessionId(list = sessions.value) {
	var home = list.find((session) => session.sessionKind === "agent" && session.canClear);
	if (home?.sessionId) return home.sessionId;
	return list[0]?.sessionId || "";
}

	export var sessionStore = {
		sessions,
		activeSessionId,
		activeSession,
		switchInProgress,
		switchGeneration,
		Session,
		setAll,
		upsert,
		fetch,
	getById,
	setActive,
		defaultSessionId,
		notify,
	};
	export { INVALID_SESSION_LABEL, PENDING_SESSION_LABEL, sessionLabelText };
