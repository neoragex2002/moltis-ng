import test from "node:test";
import assert from "node:assert/strict";

import { normalizeSearchHits } from "./session-search-normalize.js";

test("normalizeSearchHits drops hits missing displayName and warns", () => {
	var warnings = [];
	var prevWarn = console.warn;
	console.warn = (...args) => warnings.push(args);
	try {
		var hits = normalizeSearchHits([
			{ sessionId: "sess_ok", displayName: "  Main  ", snippet: "x" },
			{ sessionId: "sess_bad", displayName: "", snippet: "y" },
			{ sessionId: "sess_bad2", snippet: "z" },
		]);
		assert.deepEqual(
			hits.map((h) => ({ sessionId: h.sessionId, displayName: h.displayName })),
			[{ sessionId: "sess_ok", displayName: "Main" }],
		);
		assert.equal(warnings.length, 2);
		assert.equal(
			typeof warnings[0][0] === "string" && warnings[0][0].includes('reason_code="missing_display_name"'),
			true,
		);
		assert.deepEqual(warnings[0][1], { session_id: "sess_bad", surface: "search_hit" });
	} finally {
		console.warn = prevWarn;
	}
});

