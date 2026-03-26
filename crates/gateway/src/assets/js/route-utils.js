// ── Pure route helpers ──────────────────────────────────────
//
// Intentionally DOM-free so it can be shared by router, startup restore,
// and Node `node:test` unit tests.

export function sessionPath(sessionId) {
	return `/chats/${encodeURIComponent(sessionId)}`;
}

