// ── Preact signal bridge for shared state ─────────────────────
// Mirrors key state.js vars as Preact signals so that both imperative
// code (websocket.js) and Preact pages can coexist during migration.
//
// Signals for models, projects, sessions, selectedModelId, and
// activeSessionId moved to stores/*.js. It is re-exported
// here for backward compat with pages that still import from signals.js.

import { signal } from "@preact/signals";
import { models, selectedModelId } from "./stores/model-store.js";
import { projects } from "./stores/project-store.js";
import { activeSessionId, sessions } from "./stores/session-store.js";

export { activeSessionId, models, projects, selectedModelId, sessions };

// Signals that haven't moved to stores yet
export var connected = signal(false);
export var cachedChannels = signal(null);
export var unseenErrors = signal(0);
export var unseenWarns = signal(0);
export var sandboxInfo = signal(null);
