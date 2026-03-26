import test from "node:test";
import assert from "node:assert/strict";

import { sessionPath } from "./route-utils.js";

test("sessionPath encodes sessionId for /chats/<id>", () => {
	assert.equal(sessionPath("sess_abc123"), "/chats/sess_abc123");
	assert.equal(sessionPath("agent:default:chat-123"), "/chats/agent%3Adefault%3Achat-123");
	assert.equal(sessionPath("a:b"), "/chats/a%3Ab");
});

