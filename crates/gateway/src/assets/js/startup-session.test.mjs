import test from "node:test";
import assert from "node:assert/strict";

function createStorage(initial = {}) {
	var data = new Map(Object.entries(initial));
	return {
		getItem(key) {
			return data.has(key) ? data.get(key) : null;
		},
		setItem(key, value) {
			data.set(key, String(value));
		},
		removeItem(key) {
			data.delete(key);
		},
		clear() {
			data.clear();
		},
	};
}

globalThis.window = {
	__MOLTIS__: {
		routes: { chats: "/chats" },
	},
};
globalThis.localStorage = createStorage();

var mod = await import("./startup-session.js");

test("resolveStartupSessionId prefers URL sessionId over stored", () => {
	localStorage.clear();
	localStorage.setItem("moltis-sessionId", "sess_stored");
	assert.deepEqual(mod.resolveStartupSessionId("sess_url"), { sessionId: "sess_url", source: "url" });
});

test("resolveStartupSessionId falls back to stored when URL missing", () => {
	localStorage.clear();
	localStorage.setItem("moltis-sessionId", "sess_stored");
	assert.deepEqual(mod.resolveStartupSessionId(""), { sessionId: "sess_stored", source: "stored" });
});

test("resolveStartupSessionId returns none when URL and stored missing", () => {
	localStorage.clear();
	assert.deepEqual(mod.resolveStartupSessionId(""), { sessionId: "", source: "none" });
});

test("preferredStartupChatPath marks restore only when using stored", () => {
	localStorage.clear();
	localStorage.setItem("moltis-sessionId", "agent:default:chat-1");
	assert.deepEqual(mod.preferredStartupChatPath(""), {
		path: "/chats/agent%3Adefault%3Achat-1",
		restoreSessionId: "agent:default:chat-1",
	});
	assert.deepEqual(mod.preferredStartupChatPath("sess_url"), { path: "/chats/sess_url", restoreSessionId: "" });
});
