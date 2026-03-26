import test from "node:test";
import assert from "node:assert/strict";

import {
	INVALID_SESSION_LABEL,
	PENDING_SESSION_LABEL,
	sessionLabelText,
} from "./session-label.js";

test("sessionLabelText uses Loading… for clientOnly sessions", () => {
	assert.equal(sessionLabelText({ clientOnly: true, displayName: "ignored" }), PENDING_SESSION_LABEL);
});

test("sessionLabelText shows Invalid session when displayName missing", () => {
	assert.equal(sessionLabelText({ clientOnly: false, displayName: "" }), INVALID_SESSION_LABEL);
	assert.equal(
		sessionLabelText({ clientOnly: false, displayName: "", label: "Legacy label", sessionId: "sess_1" }),
		INVALID_SESSION_LABEL,
	);
});

test("sessionLabelText uses trimmed displayName", () => {
	assert.equal(sessionLabelText({ clientOnly: false, displayName: "Main" }), "Main");
});

