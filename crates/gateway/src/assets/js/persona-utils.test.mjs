import test from "node:test";
import assert from "node:assert/strict";

import { isAgentListLoaded, isAgentMissing } from "./persona-utils.js";

test("isAgentListLoaded requires default agent in list", () => {
assert.equal(isAgentListLoaded([]), false);
assert.equal(isAgentListLoaded(["ops"]), false);
assert.equal(isAgentListLoaded(["default"]), true);
assert.equal(isAgentListLoaded(["default", "ops"]), true);
});

test("isAgentMissing is false before list loads", () => {
assert.equal(isAgentMissing("ops", ["default", "ops"], false), false);
assert.equal(isAgentMissing("ops", [], false), false);
});

test("isAgentMissing is true only when loaded and id absent", () => {
assert.equal(isAgentMissing("", ["default"], true), false);
assert.equal(isAgentMissing("ops", ["default", "ops"], true), false);
assert.equal(isAgentMissing("ops", ["default"], true), true);
});
