export var PENDING_SESSION_LABEL = "Loading\u2026";
export var INVALID_SESSION_LABEL = "Invalid session";

export function sessionLabelText(session) {
	if (!session) return "";
	if (session.clientOnly) return PENDING_SESSION_LABEL;
	return session.displayName || INVALID_SESSION_LABEL;
}

