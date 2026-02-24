import test from "node:test";
import assert from "node:assert/strict";

import {
	parseIdentityFrontmatter,
	stripIdentityFrontmatter,
	upsertIdentityFrontmatter,
} from "./identity-frontmatter.js";

test("parseIdentityFrontmatter parses standard YAML frontmatter", () => {
	var raw = `---\nname: Jarvis\nemoji: 🤖\ncreature: robot\nvibe: chill\n---\n\n# IDENTITY.md\n`;
	var fm = parseIdentityFrontmatter(raw);
	assert.equal(fm.name, "Jarvis");
	assert.equal(fm.emoji, "🤖");
	assert.equal(fm.creature, "robot");
	assert.equal(fm.vibe, "chill");
});

test("parseIdentityFrontmatter supports CRLF newlines", () => {
	var raw =
		"---\r\nname: Jarvis\r\nemoji: 🤖\r\ncreature: robot\r\nvibe: chill\r\n---\r\n\r\n# IDENTITY.md\r\n";
	var fm = parseIdentityFrontmatter(raw);
	assert.equal(fm.name, "Jarvis");
	assert.equal(fm.emoji, "🤖");
	assert.equal(fm.creature, "robot");
	assert.equal(fm.vibe, "chill");
});

test("stripIdentityFrontmatter removes frontmatter and keeps body", () => {
	var raw = `---\nname: Jarvis\n---\n\n# IDENTITY.md\nhello\n`;
	var body = stripIdentityFrontmatter(raw);
	assert.equal(body, "\n# IDENTITY.md\nhello\n");
});

test("upsertIdentityFrontmatter preserves existing values when re-saving", () => {
	var raw = `---\nname: Jarvis\nemoji: 🤖\ncreature: robot\nvibe: chill\n---\n\n# IDENTITY.md\nhello\n`;
	var fm = parseIdentityFrontmatter(raw);
	var next = upsertIdentityFrontmatter(raw, fm);
	var fm2 = parseIdentityFrontmatter(next);
	assert.equal(fm2.name, "Jarvis");
	assert.equal(fm2.emoji, "🤖");
	assert.equal(fm2.creature, "robot");
	assert.equal(fm2.vibe, "chill");
	assert.ok(next.includes("# IDENTITY.md\nhello\n"));
});

