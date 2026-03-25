// ── SessionHeader Preact component ───────────────────────────
//
// Replaces the imperative updateChatSessionHeader() with a reactive
// Preact component reading sessionStore.activeSession.

import { html } from "htm/preact";
import { useCallback, useRef, useState } from "preact/hooks";
import { sendRpc } from "../helpers.js";
import { clearActiveSession, fetchSessions, switchSession } from "../sessions.js";
import { sessionStore } from "../stores/session-store.js";
import { confirmDialog } from "../ui.js";

function nextSessionKey(currentKey) {
	var allSessions = sessionStore.sessions.value;
	var s = allSessions.find((x) => x.sessionId === currentKey);
	if (s?.parentSessionId) return s.parentSessionId;
	var idx = allSessions.findIndex((x) => x.sessionId === currentKey);
	if (idx >= 0 && idx + 1 < allSessions.length) return allSessions[idx + 1].sessionId;
	if (idx > 0) return allSessions[idx - 1].sessionId;
	return sessionStore.defaultSessionId(allSessions);
}

export function SessionHeader() {
	var session = sessionStore.activeSession.value;
	var currentKey = sessionStore.activeSessionId.value;

	var [renaming, setRenaming] = useState(false);
	var [clearing, setClearing] = useState(false);
	var inputRef = useRef(null);

	var fullName = session ? session.displayName || session.label || session.sessionId : currentKey;
	var displayName = fullName.length > 20 ? `${fullName.slice(0, 20)}\u2026` : fullName;

	var canRename = !!session?.canRename;
	var canDelete = !!session?.canDelete;
	var canFork = !!session?.canFork;
	var canClear = !!session?.canClear;

	var startRename = useCallback(() => {
		if (!canRename) return;
		setRenaming(true);
		requestAnimationFrame(() => {
			if (inputRef.current) {
				inputRef.current.value = fullName;
				inputRef.current.focus();
				inputRef.current.select();
			}
		});
	}, [canRename, fullName]);

	var commitRename = useCallback(() => {
		var val = inputRef.current?.value.trim() || "";
		setRenaming(false);
		if (val && val !== fullName) {
			sendRpc("sessions.patch", { sessionId: currentKey, label: val }).then((res) => {
				if (res?.ok) fetchSessions();
			});
		}
	}, [currentKey, fullName]);

	var onKeyDown = useCallback(
		(e) => {
			if (e.key === "Enter") {
				e.preventDefault();
				commitRename();
			}
			if (e.key === "Escape") {
				setRenaming(false);
			}
		},
		[commitRename],
	);

	var onFork = useCallback(() => {
		sendRpc("sessions.fork", { sessionId: currentKey }).then((res) => {
			if (res?.ok && res.payload?.sessionId) {
				fetchSessions();
				switchSession(res.payload.sessionId);
			}
		});
	}, [currentKey]);

	var onDelete = useCallback(() => {
		var msgCount = session ? session.messageCount || 0 : 0;
		var nextKey = nextSessionKey(currentKey);
		var doDelete = () => {
			sendRpc("sessions.delete", { sessionId: currentKey }).then((res) => {
				if (res && !res.ok && res.error && res.error.indexOf("uncommitted changes") !== -1) {
					confirmDialog("Worktree has uncommitted changes. Force delete?").then((yes) => {
						if (!yes) return;
						sendRpc("sessions.delete", { sessionId: currentKey, force: true }).then(() => {
							switchSession(nextKey);
							fetchSessions();
						});
					});
					return;
				}
				switchSession(nextKey);
				fetchSessions();
			});
		};
		var isUnmodifiedFork = session && session.forkPoint != null && msgCount <= session.forkPoint;
		if (msgCount > 0 && !isUnmodifiedFork) {
			confirmDialog("Delete this session?").then((yes) => {
				if (yes) doDelete();
			});
		} else {
			doDelete();
		}
	}, [currentKey, session]);

	var onClear = useCallback(() => {
		if (clearing) return;
		setClearing(true);
		clearActiveSession().finally(() => {
			setClearing(false);
		});
	}, [clearing]);

	return html`
		<div class="flex items-center gap-2">
			${
				renaming
					? html`<input
						ref=${inputRef}
						class="chat-session-rename-input"
						onBlur=${commitRename}
						onKeyDown=${onKeyDown}
					/>`
					: html`<span
						class="chat-session-name"
						style=${{ cursor: canRename ? "pointer" : "default" }}
						title=${canRename ? "Click to rename" : ""}
						onClick=${startRename}
					>${displayName}</span>`
			}
			${
				canFork &&
				html`
				<button class="chat-session-btn" onClick=${onFork} title="Fork session">
					Fork
				</button>
			`
			}
			${
				canClear &&
				html`
				<button class="chat-session-btn" onClick=${onClear} title="Clear session" disabled=${clearing}>
					${clearing ? "Clearing\u2026" : "Clear"}
				</button>
			`
			}
			${
				canDelete &&
				html`
				<button class="chat-session-btn chat-session-btn-danger" onClick=${onDelete} title="Delete session">
					Delete
				</button>
			`
			}
		</div>
	`;
}
