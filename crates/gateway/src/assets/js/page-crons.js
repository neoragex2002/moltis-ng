// ── Crons page (Preact + HTM + Signals) ──────────────────────

import { signal } from "@preact/signals";
import { html } from "htm/preact";
import { render } from "preact";
import { useEffect, useLayoutEffect, useRef, useState } from "preact/hooks";
import * as gon from "./gon.js";
import { sendRpc } from "./helpers.js";
import { updateNavCount } from "./nav-counts.js";
import { navigate, registerPrefix } from "./router.js";
import { routes } from "./routes.js";
import { models as modelsSig } from "./stores/model-store.js";
import { ConfirmDialog, Modal, ModelSelect, requestConfirm } from "./ui.js";

var initialCrons = gon.get("crons") || [];
var cronJobs = signal(initialCrons);
var cronStatus = signal(gon.get("cron_status"));
if (initialCrons.length) {
	updateNavCount("crons", initialCrons.filter((j) => j.enabled).length);
}
var runsHistory = signal(null); // { jobId, jobName, runs }
var showModal = signal(false);
var editingJob = signal(null);
var activeSection = signal("jobs");
var _cronsContainer = null;
var cronsRouteBase = routes.crons;
var syncCronsRoute = true;

// ── Agents / Heartbeat (per-agent) ──────────────────────────────────────────
var agentIds = signal(null); // null | string[]
var heartbeatAgentId = signal("default");
var heartbeatStatus = signal(null); // HeartbeatStatus | null
var heartbeatRuns = signal([]);
var heartbeatSaving = signal(false);
var heartbeatRunning = signal(false);
var heartbeatError = signal("");

function normalizeAgentIds(payload) {
	if (Array.isArray(payload)) {
		if (payload.every((v) => typeof v === "string")) return payload;
		// Best-effort: [{ id: "default" }, ...]
		return payload.map((v) => v && v.id).filter((v) => typeof v === "string");
	}
	return [];
}

function loadAgents() {
	sendRpc("agents.list", {}).then((res) => {
		if (res?.ok) agentIds.value = normalizeAgentIds(res.payload);
	});
}

function heartbeatPromptPath(agentId) {
	return `agents/${agentId}/HEARTBEAT.md`;
}

function loadHeartbeatStatus(agentId) {
	var id = (agentId || heartbeatAgentId.value || "").trim();
	if (!id) return;
	sendRpc("heartbeat.status", { agentId: id }).then((res) => {
		if (!res?.ok) return;
		heartbeatStatus.value = res.payload ?? null;
	});
}

function loadHeartbeatRuns(agentId) {
	var id = (agentId || heartbeatAgentId.value || "").trim();
	if (!id) return;
	heartbeatRuns.value = null;
	sendRpc("heartbeat.runs", { agentId: id, limit: 10 }).then((res) => {
		heartbeatRuns.value = res?.ok ? res.payload || [] : [];
	});
}

function loadStatus() {
	sendRpc("cron.status", {}).then((res) => {
		if (res?.ok) cronStatus.value = res.payload;
	});
}

function loadJobs() {
	sendRpc("cron.list", {}).then((res) => {
		if (res?.ok) {
			cronJobs.value = res.payload || [];
			updateNavCount("crons", cronJobs.value.filter((j) => j.enabled).length);
		}
	});
}

function formatSchedule(sched) {
	if (!sched) return "\u2014";
	if (sched.kind === "once") return `Once at ${formatRfc3339Local(sched.at)}`;
	if (sched.kind === "every") return `Every ${sched.every}`;
	if (sched.kind === "cron") return `${sched.expr} (${sched.timezone})`;
	return JSON.stringify(sched);
}

function parseRfc3339Ms(s) {
	if (!s) return null;
	var ms = Date.parse(s);
	return Number.isNaN(ms) ? null : ms;
}

function formatRfc3339Local(s) {
	var ms = parseRfc3339Ms(s);
	if (ms == null) return s || "\u2014";
	return new Date(ms).toLocaleString();
}

function formatRfc3339Iso(s) {
	var ms = parseRfc3339Ms(s);
	if (ms == null) return s || "\u2014";
	return new Date(ms).toISOString();
}

function durationMsFromRange(start, end) {
	var a = parseRfc3339Ms(start);
	var b = parseRfc3339Ms(end);
	if (a == null || b == null) return null;
	return Math.max(0, b - a);
}

// ── Sidebar navigation ──────────────────────────────────────

var sections = [
	{
		id: "jobs",
		label: "Cron Jobs",
		icon: html`<span class="icon icon-cron"></span>`,
	},
	{
		id: "heartbeat",
		label: "Heartbeat",
		icon: html`<span class="icon icon-heart"></span>`,
	},
];

var sectionIds = sections.map((s) => s.id);

function setCronsSection(sectionId) {
	if (!sectionIds.includes(sectionId)) return;
	if (syncCronsRoute) {
		navigate(`${cronsRouteBase}/${sectionId}`);
		return;
	}
	activeSection.value = sectionId;
}

function CronsSidebar() {
	return html`<div class="settings-sidebar">
		<div class="settings-sidebar-nav">
			${sections.map(
				(s) => html`
				<button
					key=${s.id}
					class="settings-nav-item ${activeSection.value === s.id ? "active" : ""}"
					onClick=${() => setCronsSection(s.id)}
				>
					${s.icon}
					${s.label}
				</button>
			`,
			)}
		</div>
	</div>`;
}

// ── Heartbeat Card ───────────────────────────────────────────

function formatTokens(n) {
	if (n == null) return null;
	if (n >= 1000) return `${(n / 1000).toFixed(1).replace(/\.0$/, "")}K`;
	return String(n);
}

function TokenBadge({ run }) {
	if (run.inputTokens == null && run.outputTokens == null) return null;
	var parts = [];
	if (run.inputTokens != null) parts.push(`${formatTokens(run.inputTokens)} in`);
	if (run.outputTokens != null) parts.push(`${formatTokens(run.outputTokens)} out`);
	return html`<span class="text-xs text-[var(--muted)] font-mono">${parts.join(" / ")}</span>`;
}

function HeartbeatRunsList({ runs }) {
	if (runs === null) return html`<div class="text-xs text-[var(--muted)]">Loading\u2026</div>`;
	if (runs.length === 0) return html`<div class="text-xs text-[var(--muted)]">No runs yet.</div>`;
	return html`<div class="flex flex-col">
    ${runs.map(
			(
				run,
			) => {
				var dur = durationMsFromRange(run.startedAt, run.finishedAt);
				return html`<div key=${run.runId || run.startedAt} class="flex items-center gap-3 py-2 border-b border-[var(--border)]" style="min-height:36px;">
        <span class="status-dot ${run.status === "ok" ? "connected" : ""}"></span>
        <span class="cron-badge ${run.status}">${run.status}</span>
        <span class="text-xs text-[var(--muted)] font-mono">${dur == null ? "\u2014" : `${dur}ms`}</span>
        <${TokenBadge} run=${run} />
        ${run.error && html`<span class="text-xs text-[var(--error)] truncate">${run.error}</span>`}
        <span class="flex-1"></span>
        <span class="text-xs text-[var(--muted)]"><time>${formatRfc3339Iso(run.startedAt)}</time></span>
      </div>`;
			},
		)}
  </div>`;
}

function heartbeatModelPlaceholder() {
	return modelsSig.value.length > 0
		? `(default: ${modelsSig.value[0].displayName || modelsSig.value[0].id})`
		: "(server default)";
}

var systemTimezone = Intl.DateTimeFormat().resolvedOptions().timeZone;

function defaultHeartbeatDraft(agentId) {
	return {
		agentId: agentId,
		enabled: false,
		every: "30m",
		sessionTarget: { kind: "main" },
		modelSelector: { kind: "inherit" },
		activeHours: null,
	};
}

function heartbeatDraftFromStatus(agentId, status) {
	if (status && status.config && status.config.agentId === agentId) {
		return {
			...status.config,
			activeHours: status.config.activeHours || null,
		};
	}
	return defaultHeartbeatDraft(agentId);
}

function HeartbeatSection() {
	var selectedAgentId = heartbeatAgentId.value;
	var status = heartbeatStatus.value;
	var saving = heartbeatSaving.value;
	var running = heartbeatRunning.value;
	var errorText = heartbeatError.value;
	var [draft, setDraft] = useState(heartbeatDraftFromStatus(selectedAgentId, status));

	useEffect(() => {
		setDraft(heartbeatDraftFromStatus(selectedAgentId, heartbeatStatus.value));
		heartbeatError.value = "";
	}, [selectedAgentId, status]);

	function updateDraft(patch) {
		setDraft((prev) => ({ ...prev, ...patch }));
		heartbeatError.value = "";
	}

	function onChangeAgentId(e) {
		var id = (e.target.value || "").trim();
		heartbeatAgentId.value = id || "default";
		heartbeatStatus.value = null;
		heartbeatRuns.value = [];
		heartbeatError.value = "";
		loadHeartbeatStatus(id);
		loadHeartbeatRuns(id);
	}

	function onSave(e) {
		e.preventDefault();
		var agentId = (draft.agentId || "").trim();
		if (!agentId) {
			heartbeatError.value = "Missing agentId.";
			return;
		}
		var cfg = { ...draft, agentId: agentId };
		heartbeatSaving.value = true;
		sendRpc("heartbeat.update", cfg).then((res) => {
			heartbeatSaving.value = false;
			if (res?.ok) {
				heartbeatStatus.value = res.payload;
				loadHeartbeatRuns(agentId);
			} else {
				heartbeatError.value =
					(res?.error && (res.error.message || res.error.detail)) ||
					"Failed to save heartbeat config.";
			}
		});
	}

	function onRunNow() {
		var agentId = (draft.agentId || "").trim();
		if (!agentId) return;
		if (heartbeatStatus.value?.config?.agentId !== agentId) {
			heartbeatError.value = "No heartbeat config yet. Save the config first.";
			return;
		}
		heartbeatRunning.value = true;
		sendRpc("heartbeat.run", { agentId: agentId, force: true }).then((res) => {
			heartbeatRunning.value = false;
			if (!res?.ok) {
				heartbeatError.value =
					(res?.error && (res.error.message || res.error.detail)) ||
					"Failed to run heartbeat.";
				return;
			}
			loadHeartbeatStatus(agentId);
			loadHeartbeatRuns(agentId);
		});
	}

	var isExplicitModel = draft.modelSelector?.kind === "explicit";
	var modelValue = isExplicitModel ? draft.modelSelector.modelId || "" : "";
	var activeHoursEnabled = !!draft.activeHours;
	var state = status?.state;
	var promptPath = heartbeatPromptPath((draft.agentId || "").trim() || "default");

	return html`<div class="heartbeat-form" style="max-width:600px;">
    <!-- Header -->
    <div class="flex items-center justify-between mb-2">
      <div class="flex items-center gap-3">
        <h2 class="text-lg font-medium text-[var(--text-strong)]">Heartbeat</h2>
      </div>
      <button
        class="provider-btn provider-btn-secondary"
        onClick=${onRunNow}
        disabled=${running}
      >
        ${running ? "Running\u2026" : "Run Now"}
      </button>
	</div>
	<p class="text-sm text-[var(--muted)] mb-4">Periodic agent wake-up that runs in an explicit session context.</p>

    <div class="info-bar" style="margin-top:16px;margin-bottom:16px;">
      <span class="info-field">
        <span class="info-label">Agent:</span>
        <span class="info-value font-mono">${selectedAgentId || "\u2014"}</span>
      </span>
      ${
				state?.lastStatus &&
				html`<span class="info-field">
        <span class="info-label">Last:</span>
        <span class="cron-badge ${state.lastStatus}">${state.lastStatus}</span>
      </span>`
			}
      ${
				state?.nextRunAt &&
				html`<span class="info-field">
        <span class="info-label">Next:</span>
        <span class="info-value"><time>${formatRfc3339Local(state.nextRunAt)}</time></span>
      </span>`
			}
    </div>

    ${errorText && html`<div class="alert-info-text max-w-form mb-4">
      <span class="alert-label-info">Heartbeat:</span> ${errorText}
    </div>`}

    <!-- Agent -->
    <div style="margin-top:24px;border-top:1px solid var(--border);padding-top:16px;">
      <h3 class="text-sm font-medium text-[var(--text-strong)] mb-3">Agent</h3>
      <label class="block text-xs text-[var(--muted)] mb-1">Agent ID</label>
      <input class="provider-key-input font-mono" list="hbAgentIds" value=${draft.agentId || ""}
        placeholder="default"
        onInput=${(e) => {
					var nextAgentId = e.target.value;
					updateDraft({ agentId: nextAgentId });
					heartbeatAgentId.value = nextAgentId;
					if ((heartbeatStatus.value?.config?.agentId || "") !== nextAgentId.trim()) {
						heartbeatStatus.value = null;
						heartbeatRuns.value = [];
					}
				}}
        onBlur=${onChangeAgentId} />
      <datalist id="hbAgentIds">
        ${(agentIds.value || []).map((id) => html`<option value=${id} key=${id} />`)}
      </datalist>
      <p class="text-xs text-[var(--muted)] mt-2">Heartbeat prompt is owned by <code>${promptPath}</code> (not configurable here).</p>
    </div>

    <!-- Enable -->
    <div style="margin-top:24px;border-top:1px solid var(--border);padding-top:16px;">
      <h3 class="text-sm font-medium text-[var(--text-strong)] mb-3">Enable</h3>
      <label class="text-xs text-[var(--muted)] flex items-center gap-2">
        <input type="checkbox" checked=${draft.enabled !== false}
          onChange=${(e) => {
						updateDraft({ enabled: e.target.checked });
					}} />
        Enabled
      </label>
      <p class="text-xs text-[var(--muted)] mt-2">When enabled, <code>${promptPath}</code> must exist and contain actionable content (not only headers/comments).</p>
    </div>

    <!-- Schedule -->
    <div style="margin-top:24px;border-top:1px solid var(--border);padding-top:16px;">
      <h3 class="text-sm font-medium text-[var(--text-strong)] mb-3">Schedule</h3>
      <div class="grid gap-4" style="grid-template-columns:1fr 1fr;">
        <div>
          <label class="block text-xs text-[var(--muted)] mb-1">Interval</label>
          <input class="provider-key-input" placeholder="30m" value=${draft.every || "30m"}
            onInput=${(e) => updateDraft({ every: e.target.value })} />
        </div>
        <div>
          <label class="block text-xs text-[var(--muted)] mb-1">Model</label>
          <${ModelSelect} models=${modelsSig.value} value=${modelValue}
            onChange=${(v) => {
							if (v) updateDraft({ modelSelector: { kind: "explicit", modelId: v } });
							else updateDraft({ modelSelector: { kind: "inherit" } });
						}}
            placeholder=${heartbeatModelPlaceholder()} />
        </div>
      </div>
    </div>

    <!-- Session -->
    <div style="margin-top:24px;border-top:1px solid var(--border);padding-top:16px;">
      <h3 class="text-sm font-medium text-[var(--text-strong)] mb-3">Session</h3>
      <label class="block text-xs text-[var(--muted)] mb-1">Target</label>
      <select class="provider-key-input" value=${draft.sessionTarget?.kind || "main"}
        onChange=${(e) => {
					var kind = e.target.value;
					if (kind === "main") updateDraft({ sessionTarget: { kind: "main" } });
					else updateDraft({ sessionTarget: { kind: "session", sessionKey: "" } });
				}}>
        <option value="main">Main</option>
        <option value="session">Session Key</option>
      </select>
      ${draft.sessionTarget?.kind === "session" &&
			html`<div class="mt-3">
        <label class="block text-xs text-[var(--muted)] mb-1">Session Key</label>
        <input class="provider-key-input font-mono" placeholder="agent:default:main" value=${draft.sessionTarget.sessionKey || ""}
          onInput=${(e) => updateDraft({ sessionTarget: { kind: "session", sessionKey: e.target.value } })} />
      </div>`}
    </div>

    <!-- Active Hours -->
    <div style="margin-top:24px;border-top:1px solid var(--border);padding-top:16px;">
      <h3 class="text-sm font-medium text-[var(--text-strong)] mb-3">Active Hours</h3>
      <label class="text-xs text-[var(--muted)] flex items-center gap-2 mb-3">
        <input type="checkbox" checked=${activeHoursEnabled}
          onChange=${(e) => {
						if (e.target.checked) {
							updateDraft({
								activeHours: {
									start: "08:00",
									end: "24:00",
									timezone: systemTimezone || "UTC",
								},
							});
						} else {
							updateDraft({ activeHours: null });
						}
					}} />
        Limit runs to active hours
      </label>
      ${activeHoursEnabled &&
			html`<div>
      <div class="grid gap-4" style="grid-template-columns:1fr 1fr;">
        <div>
          <label class="block text-xs text-[var(--muted)] mb-1">Start</label>
          <input class="provider-key-input font-mono" placeholder="08:00" value=${draft.activeHours?.start || "08:00"}
            onInput=${(e) => updateDraft({ activeHours: { ...draft.activeHours, start: e.target.value } })} />
        </div>
        <div>
          <label class="block text-xs text-[var(--muted)] mb-1">End</label>
          <input class="provider-key-input font-mono" placeholder="24:00" value=${draft.activeHours?.end || "24:00"}
            onInput=${(e) => updateDraft({ activeHours: { ...draft.activeHours, end: e.target.value } })} />
        </div>
      </div>
      <div class="mt-3">
        <label class="block text-xs text-[var(--muted)] mb-1">Timezone</label>
        <select class="provider-key-input" value=${draft.activeHours?.timezone || systemTimezone || "UTC"}
          onChange=${(e) => updateDraft({ activeHours: { ...draft.activeHours, timezone: e.target.value } })}>
          <option value="UTC">UTC</option>
          <option value=${systemTimezone}>${systemTimezone}</option>
          <option value="America/New_York">America/New_York</option>
          <option value="America/Chicago">America/Chicago</option>
          <option value="America/Denver">America/Denver</option>
          <option value="America/Los_Angeles">America/Los_Angeles</option>
          <option value="Europe/London">Europe/London</option>
          <option value="Europe/Paris">Europe/Paris</option>
          <option value="Europe/Berlin">Europe/Berlin</option>
          <option value="Asia/Tokyo">Asia/Tokyo</option>
          <option value="Asia/Shanghai">Asia/Shanghai</option>
          <option value="Asia/Singapore">Asia/Singapore</option>
          <option value="Australia/Sydney">Australia/Sydney</option>
        </select>
      </div>
    </div>`}
    </div>

    <!-- Recent Runs -->
    <div style="margin-top:24px;border-top:1px solid var(--border);padding-top:16px;">
      <h3 class="text-sm font-medium text-[var(--text-strong)] mb-3">Recent Runs</h3>
      <${HeartbeatRunsList} runs=${heartbeatRuns.value} />
    </div>

    <!-- Save -->
    <div style="margin-top:24px;border-top:1px solid var(--border);padding-top:16px;">
      <button class="provider-btn" onClick=${onSave} disabled=${saving}>
        ${saving ? "Saving\u2026" : "Save"}
      </button>
    </div>
  </div>`;
}

// ── Cron Jobs (existing) ─────────────────────────────────────

function StatusBar() {
	var s = cronStatus.value;
	if (!s) return html`<div class="cron-status-bar">Loading\u2026</div>`;
	var parts = [
		s.running ? "Running" : "Stopped",
		`${s.jobCount} job${s.jobCount !== 1 ? "s" : ""}`,
		`${s.enabledCount} enabled`,
	];
	if (s.nextRunAt) {
		parts.push(`next: ${formatRfc3339Local(s.nextRunAt)}`);
	}
	return html`<div class="cron-status-bar">${parts.join(" \u2022 ")}</div>`;
}

function CronJobRow(props) {
	var job = props.job;

	function onToggle(e) {
		sendRpc("cron.update", {
			id: job.jobId,
			patch: { enabled: e.target.checked },
		}).then(() => {
			loadJobs();
			loadStatus();
		});
	}

	function onRun() {
		sendRpc("cron.run", { id: job.jobId, force: true }).then(() => {
			loadJobs();
			loadStatus();
		});
	}

	function onDelete() {
		requestConfirm(`Delete job '${job.name}'?`).then((yes) => {
			if (!yes) return;
			sendRpc("cron.remove", { id: job.jobId }).then(() => {
				loadJobs();
				loadStatus();
			});
		});
	}

	function onHistory() {
		runsHistory.value = { jobId: job.jobId, jobName: job.name, runs: null };
		sendRpc("cron.runs", { id: job.jobId }).then((res) => {
			if (res?.ok)
				runsHistory.value = {
					jobId: job.jobId,
					jobName: job.name,
					runs: res.payload || [],
				};
		});
	}

	return html`<tr>
    <td>${job.name}</td>
    <td class="cron-mono">${job.agentId || "\u2014"}</td>
    <td class="cron-mono">${formatSchedule(job.schedule)}</td>
    <td class="cron-mono">${job.state?.nextRunAt ? html`<time>${formatRfc3339Iso(job.state.nextRunAt)}</time>` : "\u2014"}</td>
    <td>${job.state?.lastStatus ? html`<span class="cron-badge ${job.state.lastStatus}">${job.state.lastStatus}</span>` : "\u2014"}</td>
    <td class="cron-actions">
      <button class="cron-action-btn" onClick=${() => {
				editingJob.value = job;
				showModal.value = true;
			}}>Edit</button>
      <button class="cron-action-btn" onClick=${onRun}>Run</button>
      <button class="cron-action-btn" onClick=${onHistory}>History</button>
      <button class="cron-action-btn cron-action-danger" onClick=${onDelete}>Delete</button>
    </td>
    <td>
      <label class="cron-toggle">
        <input type="checkbox" checked=${job.enabled} onChange=${onToggle} />
        <span class="cron-slider" />
      </label>
    </td>
  </tr>`;
}

function CronJobTable() {
	var jobs = cronJobs.value;
	if (jobs.length === 0) {
		return html`<div class="text-sm text-[var(--muted)]">No cron jobs configured.</div>`;
	}
	return html`<table class="cron-table">
    <thead>
      <tr>
        <th>Name</th><th>Agent</th><th>Schedule</th>
        <th>Next Run</th><th>Last Status</th><th>Actions</th><th>Enabled</th>
      </tr>
    </thead>
    <tbody>
      ${jobs.map((job) => html`<${CronJobRow} key=${job.jobId} job=${job} />`)}
    </tbody>
  </table>`;
}

function RunHistoryPanel() {
	var h = runsHistory.value;
	if (!h) return null;

	return html`<div class="mb-md">
    <div class="flex items-center justify-between mb-md">
      <span class="text-sm font-medium text-[var(--text-strong)]">Run History: ${h.jobName}</span>
      <button class="text-xs text-[var(--muted)] cursor-pointer bg-transparent border-none hover:text-[var(--text)]"
        onClick=${() => {
					runsHistory.value = null;
				}}>\u2715 Close</button>
    </div>
    ${h.runs === null && html`<div class="text-sm text-[var(--muted)]">Loading\u2026</div>`}
    ${h.runs !== null && h.runs.length === 0 && html`<div class="text-xs text-[var(--muted)]">No runs yet.</div>`}
    ${h.runs?.map(
			(run) => {
				var dur = durationMsFromRange(run.startedAt, run.finishedAt);
				return html`<div class="cron-run-item" key=${run.runId || run.startedAt}>
        <span class="text-xs text-[var(--muted)]"><time>${formatRfc3339Iso(run.startedAt)}</time></span>
        <span class="cron-badge ${run.status}">${run.status}</span>
        <span class="text-xs text-[var(--muted)]">${dur == null ? "\u2014" : `${dur}ms`}</span>
        <${TokenBadge} run=${run} />
        ${run.error && html`<span class="text-xs text-[var(--error)]">${run.error}</span>`}
      </div>`;
			},
		)}
  </div>`;
}

function defaultCronDraft() {
	return {
		agentId: "default",
		name: "",
		schedKind: "cron",
		onceAt: "",
		every: "30m",
		cronExpr: "",
		cronTimezone: systemTimezone,
		prompt: "",
		modelId: "",
		timeoutSecs: "",
		deliveryKind: "silent",
		sessionTargetKind: "main",
		sessionKey: "",
		telegramAccountKey: "",
		telegramChatId: "",
		telegramThreadId: "",
		deleteAfterRun: false,
		enabled: true,
	};
}

function toDatetimeLocalValueFromMs(ms) {
	function pad2(n) {
		return String(n).padStart(2, "0");
	}
	var d = new Date(ms);
	return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}T${pad2(d.getHours())}:${pad2(d.getMinutes())}`;
}

function cronDraftFromJob(job) {
	var draft = defaultCronDraft();
	if (!job) return draft;
	draft.agentId = job.agentId || "default";
	draft.name = job.name || "";
	draft.schedKind = job.schedule?.kind || "cron";
	if (job.schedule?.kind === "once") {
		var ms = parseRfc3339Ms(job.schedule.at);
		draft.onceAt = ms == null ? "" : toDatetimeLocalValueFromMs(ms);
	}
	if (job.schedule?.kind === "every") draft.every = job.schedule.every || "30m";
	if (job.schedule?.kind === "cron") {
		draft.cronExpr = job.schedule.expr || "";
		draft.cronTimezone = job.schedule.timezone || systemTimezone;
	}
	draft.prompt = job.prompt || "";
	if (job.modelSelector?.kind === "explicit") draft.modelId = job.modelSelector.modelId || "";
	draft.timeoutSecs = job.timeoutSecs != null ? String(job.timeoutSecs) : "";
	draft.deleteAfterRun = Boolean(job.deleteAfterRun);
	draft.enabled = job.enabled !== false;
	if (job.delivery?.kind) {
		draft.deliveryKind = job.delivery.kind;
		if (job.delivery.kind === "session") {
			draft.sessionTargetKind = job.delivery.target?.kind || "main";
			draft.sessionKey = job.delivery.target?.sessionKey || "";
		}
		if (job.delivery.kind === "telegram") {
			draft.telegramAccountKey = job.delivery.target?.accountKey || "";
			draft.telegramChatId = job.delivery.target?.chatId || "";
			draft.telegramThreadId = job.delivery.target?.threadId || "";
		}
	}
	draft.deleteAfterRun = Boolean(job.deleteAfterRun);
	draft.enabled = job.enabled !== false;
	return draft;
}

function parseScheduleDraft(draft) {
	if (draft.schedKind === "once") {
		var ts = new Date(draft.onceAt).getTime();
		if (Number.isNaN(ts)) return { error: "onceAt" };
		var at = new Date(ts).toISOString().replace(".000Z", "Z");
		return { schedule: { kind: "once", at: at } };
	}
	if (draft.schedKind === "every") {
		var every = (draft.every || "").trim();
		if (!every) return { error: "every" };
		return { schedule: { kind: "every", every: every } };
	}
	var expr = (draft.cronExpr || "").trim();
	if (!expr) return { error: "cronExpr" };
	var timezone = (draft.cronTimezone || "").trim();
	if (!timezone) return { error: "cronTimezone" };
	return { schedule: { kind: "cron", expr: expr, timezone: timezone } };
}

function parseDeliveryDraft(draft) {
	if (draft.deliveryKind === "silent") return { delivery: { kind: "silent" } };
	if (draft.deliveryKind === "session") {
		if (draft.sessionTargetKind === "main") {
			return { delivery: { kind: "session", target: { kind: "main" } } };
		}
		var sessionKey = (draft.sessionKey || "").trim();
		if (!sessionKey) return { error: "sessionKey" };
		return { delivery: { kind: "session", target: { kind: "session", sessionKey: sessionKey } } };
	}
	if (draft.deliveryKind === "telegram") {
		var accountKey = (draft.telegramAccountKey || "").trim();
		var chatId = (draft.telegramChatId || "").trim();
		var threadId = (draft.telegramThreadId || "").trim();
		if (!accountKey) return { error: "telegramAccountKey" };
		if (!chatId) return { error: "telegramChatId" };
		var target = { accountKey: accountKey, chatId: chatId };
		if (threadId) target.threadId = threadId;
		return { delivery: { kind: "telegram", target: target } };
	}
	return { error: "deliveryKind" };
}

function CronModal() {
	var isEdit = !!editingJob.value;
	var job = editingJob.value;
	var [draft, setDraft] = useState(defaultCronDraft());
	var [saving, setSaving] = useState(false);
	var [errorField, setErrorField] = useState(null);
	var [error, setError] = useState("");
	var requestVersionRef = useRef(0);

	useLayoutEffect(() => {
		if (!showModal.value) return;
		setDraft(isEdit ? cronDraftFromJob(job) : defaultCronDraft());
		setSaving(false);
		setErrorField(null);
		setError("");
	}, [showModal.value, job?.jobId]);

	function updateDraft(patch) {
		setDraft((prev) => ({ ...prev, ...patch }));
		setErrorField(null);
		setError("");
	}

	function closeModal() {
		requestVersionRef.current += 1;
		showModal.value = false;
		editingJob.value = null;
		setDraft(defaultCronDraft());
		setSaving(false);
		setErrorField(null);
		setError("");
	}

	function onSave(e) {
		e.preventDefault();
		setError("");
		var agentId = (draft.agentId || "").trim();
		if (!agentId) {
			setErrorField("agentId");
			return;
		}
		var name = draft.name.trim();
		if (!name) {
			setErrorField("name");
			return;
		}
		var parsed = parseScheduleDraft(draft);
		if (parsed.error) {
			setErrorField(parsed.error);
			return;
		}
		var promptText = (draft.prompt || "").trim();
		if (!promptText) {
			setErrorField("prompt");
			return;
		}
		var parsedDelivery = parseDeliveryDraft(draft);
		if (parsedDelivery.error) {
			setErrorField(parsedDelivery.error);
			return;
		}
		setErrorField(null);
		setError("");
		var timeoutSecs = null;
		var timeoutRaw = (draft.timeoutSecs || "").trim();
		if (timeoutRaw) {
			var parsedTimeout = parseInt(timeoutRaw, 10);
			if (Number.isNaN(parsedTimeout) || parsedTimeout <= 0) {
				setErrorField("timeoutSecs");
				return;
			}
			timeoutSecs = parsedTimeout;
		}
		var modelSelector = draft.modelId
			? { kind: "explicit", modelId: draft.modelId }
			: { kind: "inherit" };
		var fields = {
			agentId: agentId,
			name: name,
			schedule: parsed.schedule,
			prompt: promptText,
			modelSelector: modelSelector,
			timeoutSecs: timeoutSecs,
			delivery: parsedDelivery.delivery,
			deleteAfterRun: draft.deleteAfterRun,
			enabled: draft.enabled,
		};

		setSaving(true);
		var requestId = requestVersionRef.current + 1;
		requestVersionRef.current = requestId;
		var rpcMethod = isEdit ? "cron.update" : "cron.add";
		var rpcParams = isEdit ? { id: job.jobId, patch: fields } : fields;
		sendRpc(rpcMethod, rpcParams).then((res) => {
			if (res?.ok) {
				loadJobs();
				loadStatus();
			}
			if (requestVersionRef.current !== requestId) return;
			setSaving(false);
			if (res?.ok) {
				closeModal();
			} else {
				setError((res?.error && (res.error.message || res.error.detail)) || "Failed to save cron job.");
			}
		});
	}

	function schedParams() {
		if (draft.schedKind === "once") {
			return html`<input data-field="onceAt" class="provider-key-input" type="datetime-local"
        value=${draft.onceAt}
        onInput=${(e) => {
					updateDraft({ onceAt: e.target.value });
				}} />`;
		}
		if (draft.schedKind === "every") {
			return html`<input data-field="every" class="provider-key-input" placeholder="30m"
        value=${draft.every}
        onInput=${(e) => {
					updateDraft({ every: e.target.value });
				}} />`;
		}
		return html`
      <input data-field="cronExpr" class="provider-key-input" placeholder="0 9 * * *"
        value=${draft.cronExpr}
        onInput=${(e) => {
					updateDraft({ cronExpr: e.target.value });
				}} />
      <input data-field="cronTimezone" class="provider-key-input" placeholder=${systemTimezone}
        value=${draft.cronTimezone}
        onInput=${(e) => {
					updateDraft({ cronTimezone: e.target.value });
				}} />
    `;
	}

	function deliveryParams() {
		if (draft.deliveryKind === "session") {
			return html`
        <label class="text-xs text-[var(--muted)]">Session Target</label>
        <select class="provider-key-input" value=${draft.sessionTargetKind}
          onChange=${(e) => updateDraft({ sessionTargetKind: e.target.value })}>
          <option value="main">Main</option>
          <option value="session">Session Key</option>
        </select>
        ${draft.sessionTargetKind === "session" &&
				html`<input class="provider-key-input font-mono ${errorField === "sessionKey" ? "field-error" : ""}"
            placeholder="agent:default:main"
            value=${draft.sessionKey}
            onInput=${(e) => updateDraft({ sessionKey: e.target.value })} />`}
      `;
		}
		if (draft.deliveryKind === "telegram") {
			return html`
        <label class="text-xs text-[var(--muted)]">Telegram Account Key</label>
        <input class="provider-key-input font-mono ${errorField === "telegramAccountKey" ? "field-error" : ""}"
          placeholder="telegram:123456789"
          value=${draft.telegramAccountKey}
          onInput=${(e) => updateDraft({ telegramAccountKey: e.target.value })} />
        <label class="text-xs text-[var(--muted)]">Chat ID</label>
        <input class="provider-key-input font-mono ${errorField === "telegramChatId" ? "field-error" : ""}"
          placeholder="-1001234567890"
          value=${draft.telegramChatId}
          onInput=${(e) => updateDraft({ telegramChatId: e.target.value })} />
        <label class="text-xs text-[var(--muted)]">Thread ID (optional)</label>
        <input class="provider-key-input font-mono"
          placeholder="123"
          value=${draft.telegramThreadId}
          onInput=${(e) => updateDraft({ telegramThreadId: e.target.value })} />
      `;
		}
		return null;
	}

	return html`<${Modal} show=${showModal.value} onClose=${closeModal} title=${isEdit ? "Edit Job" : "Add Job"}>
    <div class="provider-key-form">
      <label class="text-xs text-[var(--muted)]">Agent ID</label>
      <input data-field="agentId" class="provider-key-input font-mono ${errorField === "agentId" ? "field-error" : ""}"
        list="cronAgentIds"
        placeholder="default" value=${draft.agentId}
        onInput=${(e) => {
					updateDraft({ agentId: e.target.value });
				}} />
      <datalist id="cronAgentIds">
        ${(agentIds.value || []).map((id) => html`<option value=${id} key=${id} />`)}
      </datalist>

      <label class="text-xs text-[var(--muted)]">Name</label>
      <input data-field="name" class="provider-key-input ${errorField === "name" ? "field-error" : ""}"
        placeholder="Job name" value=${draft.name}
        onInput=${(e) => {
					updateDraft({ name: e.target.value });
				}} />

      <label class="text-xs text-[var(--muted)]">Schedule Type</label>
      <select data-field="schedKind" class="provider-key-input" value=${draft.schedKind}
        onChange=${(e) => {
					updateDraft({ schedKind: e.target.value });
				}}>
        <option value="once">Once (one-shot)</option>
        <option value="every">Every (interval)</option>
        <option value="cron">Cron (expression)</option>
      </select>

      ${schedParams()}

      <label class="text-xs text-[var(--muted)]">Prompt</label>
      <textarea data-field="prompt" class="provider-key-input textarea-sm ${errorField === "prompt" ? "field-error" : ""}"
        value=${draft.prompt}
        placeholder="What should the agent do when this cron fires?"
        onInput=${(e) => {
					updateDraft({ prompt: e.target.value });
				}} />

      <label class="text-xs text-[var(--muted)]">Model</label>
      <${ModelSelect} models=${modelsSig.value} value=${draft.modelId}
        onChange=${(v) => updateDraft({ modelId: v })}
        placeholder="inherit" />

      <label class="text-xs text-[var(--muted)]">Timeout (secs, optional)</label>
      <input data-field="timeoutSecs" class="provider-key-input ${errorField === "timeoutSecs" ? "field-error" : ""}"
        type="number" min="1" placeholder="60"
        value=${draft.timeoutSecs}
        onInput=${(e) => updateDraft({ timeoutSecs: e.target.value })} />

      <label class="text-xs text-[var(--muted)]">Delivery</label>
      <select data-field="deliveryKind" class="provider-key-input" value=${draft.deliveryKind}
        onChange=${(e) => updateDraft({ deliveryKind: e.target.value })}>
        <option value="silent">Silent</option>
        <option value="session">Session</option>
        <option value="telegram">Telegram</option>
      </select>
      ${deliveryParams()}

      <label class="text-xs text-[var(--muted)] flex items-center gap-2">
        <input data-field="deleteAfter" type="checkbox" checked=${draft.deleteAfterRun}
          onChange=${(e) => {
					updateDraft({ deleteAfterRun: e.target.checked });
				}} />
        Delete after run
      </label>
      <label class="text-xs text-[var(--muted)] flex items-center gap-2">
        <input data-field="enabled" type="checkbox" checked=${draft.enabled}
          onChange=${(e) => {
					updateDraft({ enabled: e.target.checked });
				}} />
        Enabled
      </label>
      ${error && html`<div class="text-xs text-[var(--error)]">${error}</div>`}

      <div class="btn-row-mt">
        <button class="provider-btn provider-btn-secondary" onClick=${() => {
					closeModal();
				}}>Cancel</button>
        <button class="provider-btn" onClick=${onSave} disabled=${saving}>
          ${saving ? "Saving\u2026" : isEdit ? "Update" : "Create"}
        </button>
      </div>
    </div>
  </${Modal}>`;
}

// ── Section content panels ──────────────────────────────────

function HeartbeatPanel() {
	useEffect(() => {
		loadAgents();
		loadHeartbeatStatus(heartbeatAgentId.value);
		loadHeartbeatRuns(heartbeatAgentId.value);
	}, []);

	return html`<div class="p-6">
    <${HeartbeatSection} />
  </div>`;
}

function CronJobsPanel() {
	useEffect(() => {
		loadStatus();
		loadJobs();
	}, []);

	return html`<div class="p-4 flex flex-col gap-4">
    <div class="flex items-center gap-3">
      <h2 class="text-lg font-medium text-[var(--text-strong)]">Cron Jobs</h2>
      <button class="provider-btn"
        onClick=${() => {
					editingJob.value = null;
					showModal.value = true;
				}}>+ Add Job</button>
    </div>
    <${StatusBar} />
    <${CronJobTable} />
    <${RunHistoryPanel} />
  </div>`;
}

// ── Main page ───────────────────────────────────────────────

function CronsPage() {
	return html`
    <div class="settings-layout">
      <${CronsSidebar} />
      <div class="flex-1 overflow-y-auto">
        ${activeSection.value === "jobs" && html`<${CronJobsPanel} />`}
        ${activeSection.value === "heartbeat" && html`<${HeartbeatPanel} />`}
      </div>
    </div>
    <${CronModal} />
    <${ConfirmDialog} />
  `;
}

registerPrefix(routes.crons, initCrons, teardownCrons);

export function initCrons(container, param, options) {
	_cronsContainer = container;
	cronsRouteBase = options?.routeBase || routes.crons;
	syncCronsRoute = options?.syncRoute !== false;

	container.style.cssText = "flex-direction:row;padding:0;overflow:hidden;";
	cronJobs.value = gon.get("crons") || [];
	cronStatus.value = gon.get("cron_status");
	runsHistory.value = null;
	showModal.value = false;
	editingJob.value = null;
	agentIds.value = null;
	heartbeatAgentId.value = "default";
	heartbeatStatus.value = null;
	heartbeatRuns.value = [];
	heartbeatSaving.value = false;
	heartbeatRunning.value = false;
	heartbeatError.value = "";

	var section = param && sectionIds.includes(param) ? param : "jobs";
	if (syncCronsRoute && param && !sectionIds.includes(param)) {
		history.replaceState(null, "", `${cronsRouteBase}/jobs`);
	}
	activeSection.value = section;

	// Eagerly load heartbeat data so it's ready when the panel mounts.
	loadAgents();
	loadHeartbeatStatus(heartbeatAgentId.value);
	loadHeartbeatRuns(heartbeatAgentId.value);

	render(html`<${CronsPage} />`, container);
}

export function teardownCrons() {
	if (_cronsContainer) render(null, _cronsContainer);
	_cronsContainer = null;
	cronsRouteBase = routes.crons;
	syncCronsRoute = true;
}
