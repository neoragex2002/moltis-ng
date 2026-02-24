import test from "node:test";
import assert from "node:assert/strict";

import { isPersonaListLoaded, isPersonaMissing } from "./persona-utils.js";

test("isPersonaListLoaded requires default persona in list", () => {
	assert.equal(isPersonaListLoaded([]), false);
	assert.equal(isPersonaListLoaded(["ops"]), false);
	assert.equal(isPersonaListLoaded(["default"]), true);
	assert.equal(isPersonaListLoaded(["default", "ops"]), true);
});

test("isPersonaMissing is false before list loads", () => {
	assert.equal(isPersonaMissing("ops", ["default", "ops"], false), false);
	assert.equal(isPersonaMissing("ops", [], false), false);
});

test("isPersonaMissing is true only when loaded and id absent", () => {
	assert.equal(isPersonaMissing("", ["default"], true), false);
	assert.equal(isPersonaMissing("ops", ["default", "ops"], true), false);
	assert.equal(isPersonaMissing("ops", ["default"], true), true);
});

