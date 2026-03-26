import { sessionPath } from "./route-utils.js";
import { routes } from "./routes.js";

export function readStoredSessionId() {
	return localStorage.getItem("moltis-sessionId") || "";
}

/**
 * Resolve the initial sessionId for chat entrypoints.
 *
 * Rules (one-cut, no pre-bootstrap existence checks):
 * - URL sessionId wins.
 * - Otherwise, use localStorage.moltis-sessionId if present.
 * - Otherwise, leave empty and let the chat page resolve via sessions.home.
 */
export function resolveStartupSessionId(urlSessionId) {
	if (urlSessionId) return { sessionId: urlSessionId, source: "url" };
	var storedSessionId = readStoredSessionId();
	if (storedSessionId) return { sessionId: storedSessionId, source: "stored" };
	return { sessionId: "", source: "none" };
}

export function preferredStartupChatPath(urlSessionId) {
	var resolved = resolveStartupSessionId(urlSessionId);
	var path = resolved.sessionId ? sessionPath(resolved.sessionId) : routes.chats;
	var restoreSessionId = resolved.source === "stored" ? resolved.sessionId : "";
	return { path, restoreSessionId };
}
