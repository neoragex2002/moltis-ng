// ── Channels page (Preact + HTM + Signals) ──────────────────

import { signal, useSignal } from "@preact/signals";
import { html } from "htm/preact";
import { render } from "preact";
import { useEffect } from "preact/hooks";
import { onEvent } from "./events.js";
import { sendRpc } from "./helpers.js";
import { updateNavCount } from "./nav-counts.js";
import { connected } from "./signals.js";
import * as S from "./state.js";
import { models as modelsSig } from "./stores/model-store.js";
import { ConfirmDialog, Modal, ModelSelect, requestConfirm, showToast } from "./ui.js";

var channels = signal([]);
var agentNames = signal([]);
var agentNamesLoaded = signal(false);

function isAgentListLoaded(names) {
	return Array.isArray(names) && names.includes("default");
}

export function prefetchChannels() {
	sendRpc("channels.status", {}).then((res) => {
		if (res?.ok) {
			var ch = res.payload?.channels || [];
			channels.value = ch;
			S.setCachedChannels(ch);
		}
	});
	sendRpc("workspace.agent.list", {}).then((res) => {
		if (res?.ok) {
			var ids = (res.payload?.agents || []).map((p) => p.name).filter(Boolean);
			agentNames.value = ids;
			agentNamesLoaded.value = isAgentListLoaded(ids);
		} else {
			agentNamesLoaded.value = false;
		}
	});
}
var senders = signal([]);
var activeTab = signal("channels");
var showAddModal = signal(false);
var editingChannel = signal(null);
var sendersAccount = signal("");

function loadChannels() {
	sendRpc("channels.status", {}).then((res) => {
		if (res?.ok) {
			var ch = res.payload?.channels || [];
			channels.value = ch;
			S.setCachedChannels(ch);
			updateNavCount("channels", ch.length);
		}
	});
	sendRpc("workspace.agent.list", {}).then((res) => {
		if (res?.ok) {
			var ids = (res.payload?.agents || []).map((p) => p.name).filter(Boolean);
			agentNames.value = ids;
			agentNamesLoaded.value = isAgentListLoaded(ids);
		} else {
			agentNamesLoaded.value = false;
		}
	});
}

function loadSenders() {
	var chanAccountKey = sendersAccount.value;
	if (!chanAccountKey) {
		senders.value = [];
		return;
	}
	sendRpc("channels.senders.list", { chanAccountKey }).then((res) => {
		if (res?.ok) senders.value = res.payload?.senders || [];
	});
}

// ── Telegram icon (CSS mask-image) ──────────────────────────
function TelegramIcon() {
	return html`<span class="icon icon-telegram"></span>`;
}

// ── Channel card ─────────────────────────────────────────────
	function ChannelCard(props) {
		var ch = props.channel;
		var cfg = ch.config || {};
		var configuredAgent = cfg.agent_id || "";
	var agentMissing = Boolean(
		configuredAgent &&
			agentNamesLoaded.value &&
			Array.isArray(agentNames.value) &&
			!agentNames.value.includes(configuredAgent),
	);

	function copyText(label, text) {
		if (!text) return;
		if (navigator?.clipboard?.writeText) {
			navigator.clipboard
				.writeText(text)
				.then(() => showToast(`${label} copied`))
				.catch(() => showToast("Copy failed"));
		} else {
			showToast("Clipboard not available");
		}
	}

	function DetailRow({ label, value }) {
		var v = value == null ? "" : String(value);
		return html`<div style="display:flex;align-items:center;gap:8px;justify-content:space-between;">
			<div class="text-xs text-[var(--muted)]" style="min-width:140px;">${label}</div>
			<div style="display:flex;align-items:center;gap:8px;min-width:0;">
				<code class="text-xs" style="white-space:nowrap;overflow:hidden;text-overflow:ellipsis;max-width:420px;">${v || "-"}</code>
				<button
					type="button"
					class="provider-btn provider-btn-sm provider-btn-secondary"
					disabled=${!v}
					onClick=${() => copyText(label, v)}
				>
					Copy
				</button>
			</div>
		</div>`;
	}

	function onRemove() {
		requestConfirm(`Remove ${ch.name || ch.chanAccountKey}?`).then((yes) => {
			if (!yes) return;
			sendRpc("channels.remove", { chanAccountKey: ch.chanAccountKey }).then((r) => {
				if (r?.ok) loadChannels();
			});
		});
	}

	var statusClass = ch.status === "connected" ? "configured" : "oauth";
	var sessionLine = "";
	if (ch.sessions && ch.sessions.length > 0) {
		var active = ch.sessions.filter((s) => s.active);
		sessionLine =
			active.length > 0
				? active.map((s) => `${s.label || s.sessionId} (${s.messageCount} msgs)`).join(", ")
				: "No active session";
	}

	return html`<div class="provider-card" style="padding:12px 14px;border-radius:8px;margin-bottom:8px;">
    <div style="display:flex;align-items:center;gap:10px;">
      <span style="display:inline-flex;align-items:center;justify-content:center;width:28px;height:28px;border-radius:6px;background:var(--surface2);">
        <${TelegramIcon} />
      </span>
      <div style="display:flex;flex-direction:column;gap:2px;">
					<span class="text-sm text-[var(--text-strong)]">${ch.name || ch.chanAccountKey || "Telegram"}</span>
        ${ch.details && html`<span class="text-xs text-[var(--muted)]">${ch.details}</span>`}
        ${sessionLine && html`<span class="text-xs text-[var(--muted)]">${sessionLine}</span>`}
      </div>
      <span class="provider-item-badge ${statusClass}">${ch.status || "unknown"}</span>
    </div>
    <div class="flex gap-2">
					<button class="provider-btn provider-btn-sm provider-btn-secondary" title="Edit ${ch.chanAccountKey || "channel"}"
        onClick=${() => {
					editingChannel.value = ch;
				}}>Edit</button>
					<button class="provider-btn provider-btn-sm provider-btn-danger" title="Remove ${ch.chanAccountKey || "channel"}"
        onClick=${onRemove}>Remove</button>
    </div>
		<details style="margin-top:10px;">
			<summary class="text-xs text-[var(--muted)]" style="cursor:pointer;user-select:none;">Details</summary>
			<div style="margin-top:8px;display:flex;flex-direction:column;gap:6px;">
				<${DetailRow} label="chanAccountKey" value=${ch.chanAccountKey} />
				<${DetailRow} label="chanUserId" value=${cfg.chan_user_id} />
				<${DetailRow} label="chanUserName" value=${cfg.chan_user_name ? "@" + cfg.chan_user_name : ""} />
				<${DetailRow} label="chanNickname" value=${cfg.chan_nickname} />
				<${DetailRow}
					label="agentName"
					value=${agentMissing ? `Missing: ${configuredAgent} (defaults to default)` : configuredAgent}
				/>
			</div>
		</details>
  </div>`;
}

// ── Channels tab ─────────────────────────────────────────────
function ChannelsTab() {
	if (channels.value.length === 0) {
		return html`<div style="text-align:center;padding:40px 0;">
      <div class="text-sm text-[var(--muted)]" style="margin-bottom:12px;">No Telegram bots connected.</div>
      <div class="text-xs text-[var(--muted)]">Click "+ Add Telegram Bot" to connect one using a token from @BotFather.</div>
    </div>`;
	}
	return html`${channels.value.map((ch) => html`<${ChannelCard} key=${ch.chanAccountKey} channel=${ch} />`)}`;
}

// ── Senders tab ──────────────────────────────────────────────
function SendersTab() {
	useEffect(() => {
		if (channels.value.length > 0 && !sendersAccount.value) {
			sendersAccount.value = channels.value[0].chanAccountKey;
		}
	}, [channels.value]);

	useEffect(() => {
		loadSenders();
	}, [sendersAccount.value]);

	if (channels.value.length === 0) {
		return html`<div class="text-sm text-[var(--muted)]">No channels configured.</div>`;
	}

	function onAction(identifier, action) {
		var rpc = action === "approve" ? "channels.senders.approve" : "channels.senders.deny";
		sendRpc(rpc, {
			chanAccountKey: sendersAccount.value,
			identifier: identifier,
		}).then(() => {
			loadSenders();
			loadChannels();
		});
	}

	return html`<div>
    <div style="margin-bottom:12px;">
      <label class="text-xs text-[var(--muted)]" style="margin-right:6px;">Account:</label>
      <select style="background:var(--surface2);color:var(--text);border:1px solid var(--border);border-radius:4px;padding:4px 8px;font-size:12px;"
        value=${sendersAccount.value} onChange=${(e) => {
					sendersAccount.value = e.target.value;
				}}>
			${channels.value.map(
				(ch) => html`<option key=${ch.chanAccountKey} value=${ch.chanAccountKey}>${ch.name || ch.chanAccountKey}</option>`,
			)}
      </select>
    </div>
    ${senders.value.length === 0 && html`<div class="text-sm text-[var(--muted)] senders-empty">No messages received yet for this account.</div>`}
    ${
			senders.value.length > 0 &&
			html`<table class="senders-table">
      <thead><tr>
        <th class="senders-th">Sender</th><th class="senders-th">Username</th>
        <th class="senders-th">Messages</th><th class="senders-th">Last Seen</th>
        <th class="senders-th">Status</th><th class="senders-th">Action</th>
      </tr></thead>
      <tbody>
        ${senders.value.map((s) => {
					var identifier = s.username || s.peerId;
					var lastSeenMs = s.lastSeen ? s.lastSeen * 1000 : 0;
					return html`<tr key=${s.peerId}>
						<td class="senders-td">${s.senderName || s.peerId}</td>
						<td class="senders-td" style="color:var(--muted);">${s.username ? `@${s.username}` : "\u2014"}</td>
						<td class="senders-td">${s.messageCount}</td>
						<td class="senders-td" style="color:var(--muted);font-size:12px;">${lastSeenMs ? html`<time data-epoch-ms="${lastSeenMs}">${new Date(lastSeenMs).toISOString()}</time>` : "\u2014"}</td>
						<td class="senders-td">
							${
								s.otpPending
									? html`<span class="provider-item-badge cursor-pointer select-none" style="background:var(--warning-bg, #fef3c7);color:var(--warning-text, #92400e);" onClick=${() => {
											navigator.clipboard.writeText(s.otpPending.code).then(() => showToast("OTP code copied"));
										}}>OTP: <code class="text-xs">${s.otpPending.code}</code></span>`
									: html`<span class="provider-item-badge ${s.allowed ? "configured" : "oauth"}">${s.allowed ? "Allowed" : "Denied"}</span>`
							}
						</td>
            <td class="senders-td">
              ${
								s.allowed
									? html`<button class="provider-btn provider-btn-sm provider-btn-danger" onClick=${() => onAction(identifier, "deny")}>Deny</button>`
									: html`<button class="provider-btn provider-btn-sm" onClick=${() => onAction(identifier, "approve")}>Approve</button>`
							}
            </td>
          </tr>`;
				})}
      </tbody>
    </table>`
		}
  </div>`;
}

// ── Tag-style allowlist input ────────────────────────────────
function AllowlistInput({ value, onChange }) {
	var input = useSignal("");

	function addTag(raw) {
		var tag = raw.trim().replace(/^@/, "");
		if (tag && !value.includes(tag)) onChange([...value, tag]);
		input.value = "";
	}

	function removeTag(tag) {
		onChange(value.filter((t) => t !== tag));
	}

	function onKeyDown(e) {
		if (e.key === "Enter" || e.key === ",") {
			e.preventDefault();
			if (input.value.trim()) addTag(input.value);
		} else if (e.key === "Backspace" && !input.value && value.length > 0) {
			onChange(value.slice(0, -1));
		}
	}

	return html`<div class="flex flex-wrap items-center gap-1.5 rounded border border-[var(--border)] bg-[var(--surface2)] px-2 py-1.5"
    style="min-height:38px;cursor:text;"
    onClick=${(e) => e.currentTarget.querySelector("input")?.focus()}>
    ${value.map(
			(tag) => html`<span key=${tag}
        class="inline-flex items-center gap-1 rounded-full bg-[var(--accent)]/10 px-2 py-0.5 text-xs text-[var(--accent)]">
        ${tag}
        <button type="button" class="inline-flex items-center text-[var(--muted)] hover:text-[var(--accent)]"
          style="line-height:1;font-size:14px;padding:0;background:none;border:none;cursor:pointer;"
          onClick=${(e) => {
						e.stopPropagation();
						removeTag(tag);
					}}>\u00d7</button>
      </span>`,
		)}
    <input type="text" value=${input.value}
      onInput=${(e) => {
				input.value = e.target.value;
			}}
      onKeyDown=${onKeyDown}
      placeholder=${value.length === 0 ? "Type a username and press Enter" : ""}
      class="flex-1 bg-transparent text-[var(--text)] text-sm outline-none border-none"
      style="min-width:80px;padding:2px 0;font-family:var(--font-body);" />
	</div>`;
}

function defaultAddChannelDraft() {
	return {
		agent_id: "",
		token: "",
		dm_policy: "open",
		group_line_start_mention_dispatch: true,
		group_reply_to_dispatch: true,
		model: "",
		allowlist: [],
	};
}

function editChannelDraftFromChannel(ch) {
	var cfg = ch?.config || {};
	return {
		agent_id: cfg.agent_id || "",
		dm_policy: cfg.dm_policy || "open",
		group_line_start_mention_dispatch: cfg.group_line_start_mention_dispatch !== false,
		group_reply_to_dispatch: cfg.group_reply_to_dispatch !== false,
		model: cfg.model || "",
		allowlist: Array.isArray(cfg.allowlist) ? cfg.allowlist.slice() : [],
	};
}

function resolveModelProvider(modelId) {
	if (!modelId) return null;
	var found = modelsSig.value.find((item) => item.id === modelId);
	return found?.provider || null;
}

function buildModelUpdateFields(nextModelId, baseConfig) {
	var modelId = nextModelId || "";
	var previousModelId = baseConfig?.model || "";
	if (!modelId) {
		return { model: null, model_provider: null };
	}
	if (modelId !== previousModelId) {
		return { model: modelId, model_provider: resolveModelProvider(modelId) };
	}
	var fields = { model: modelId };
	if (Object.prototype.hasOwnProperty.call(baseConfig || {}, "model_provider")) {
		fields.model_provider = baseConfig.model_provider ?? null;
	}
	return fields;
}

// ── Add channel modal ────────────────────────────────────────
function AddChannelModal() {
	var draft = useSignal(defaultAddChannelDraft());
	var error = useSignal("");
	var saving = useSignal(false);
	var requestVersion = useSignal(0);

	function updateDraft(patch) {
		draft.value = { ...draft.value, ...patch };
	}

	function resetModal() {
		draft.value = defaultAddChannelDraft();
		error.value = "";
		saving.value = false;
	}

	function closeModal() {
		requestVersion.value += 1;
		showAddModal.value = false;
		resetModal();
	}

	function onSubmit(e) {
		e.preventDefault();
		var token = draft.value.token.trim();
		if (!token) {
			error.value = "Bot token is required.";
			return;
		}
		error.value = "";
		saving.value = true;
		var requestId = requestVersion.value + 1;
		requestVersion.value = requestId;
		var addConfig = {
			token: token,
			dm_policy: draft.value.dm_policy,
			group_line_start_mention_dispatch: draft.value.group_line_start_mention_dispatch,
			group_reply_to_dispatch: draft.value.group_reply_to_dispatch,
			allowlist: draft.value.allowlist.slice(),
		};
		if (draft.value.agent_id) {
			addConfig.agent_id = draft.value.agent_id;
		}
		if (draft.value.model) {
			addConfig.model = draft.value.model;
			addConfig.model_provider = resolveModelProvider(draft.value.model);
		}
		sendRpc("channels.add", {
			type: "telegram",
			config: addConfig,
		}).then((res) => {
			if (res?.ok) {
				loadChannels();
			}
			if (requestVersion.value !== requestId) return;
			saving.value = false;
			if (res?.ok) {
				closeModal();
			} else {
				error.value = (res?.error && (res.error.message || res.error.detail)) || "Failed to connect bot.";
			}
		});
	}

	var defaultPlaceholder =
		modelsSig.value.length > 0
			? `(default: ${modelsSig.value[0].displayName || modelsSig.value[0].id})`
			: "(server default)";

	var selectStyle =
		"font-family:var(--font-body);background:var(--surface2);color:var(--text);border:1px solid var(--border);border-radius:4px;padding:8px 12px;font-size:.85rem;cursor:pointer;";
	var inputStyle =
		"font-family:var(--font-body);background:var(--surface2);color:var(--text);border:1px solid var(--border);border-radius:4px;padding:8px 12px;font-size:.85rem;";

	return html`<${Modal} show=${showAddModal.value} onClose=${closeModal} title="Add Telegram Bot">
    <div class="channel-form">
      <div class="channel-card">
        <span class="text-xs font-medium text-[var(--text-strong)]">How to create a Telegram bot</span>
        <div class="text-xs text-[var(--muted)] channel-help">1. Open <a href="https://t.me/BotFather" target="_blank" class="text-[var(--accent)]" style="text-decoration:underline;">@BotFather</a> in Telegram</div>
        <div class="text-xs text-[var(--muted)]">2. Send /newbot and follow the prompts to choose a name and username</div>
        <div class="text-xs text-[var(--muted)]">3. Copy the bot token (looks like 123456:ABC-DEF...) and paste it below</div>
        <div class="text-xs text-[var(--muted)] channel-help" style="margin-top:2px;">See the <a href="https://core.telegram.org/bots/tutorial" target="_blank" class="text-[var(--accent)]" style="text-decoration:underline;">Telegram Bot Tutorial</a> for more details.</div>
      </div>
	      <label class="text-xs text-[var(--muted)]">Agent (optional)</label>
					<div class="text-xs text-[var(--muted)]" style="margin-top:-2px;margin-bottom:6px;">
						Choose an agent under <code>${"agents/<agent_id>/"}</code>.
					</div>
	      <select data-field="agentName" style=${selectStyle} name="telegram_agent_name" value=${draft.value.agent_id}
	        onChange=${(e) => {
					updateDraft({ agent_id: e.target.value });
				}}>
					<option value="">(default)</option>
					${agentNames.value.map((id) => html`<option key=${id} value=${id}>${id}</option>`)}
				</select>
				${
					!agentNamesLoaded.value
						? html`<div class="text-xs text-[var(--muted)]" style="margin-top:4px;">Loading agents\u2026</div>`
						: null
				}
	      <label class="text-xs text-[var(--muted)]">Bot Token (from @BotFather)</label>
	      <input data-field="token" type="password" placeholder="123456:ABC-DEF..." style=${inputStyle}
	        value=${draft.value.token}
	        onInput=${(e) => {
					updateDraft({ token: e.target.value });
				}}
	        autocomplete="new-password"
	        autocapitalize="none"
	        autocorrect="off"
	        spellcheck="false"
	        name="telegram_bot_token" />
      <label class="text-xs text-[var(--muted)]">DM Policy</label>
      <select data-field="dmPolicy" style=${selectStyle} value=${draft.value.dm_policy}
        onChange=${(e) => {
					updateDraft({ dm_policy: e.target.value });
				}}>
        <option value="open">Open (anyone)</option>
        <option value="allowlist">Allowlist only</option>
        <option value="disabled">Disabled</option>
      </select>
	      <label class="text-xs text-[var(--muted)]">Group Dispatch</label>
      <label class="text-xs text-[var(--muted)]" style="display:flex;align-items:center;gap:8px;margin-top:4px;">
        <input type="checkbox" data-field="groupLineStartMentionDispatch" checked=${draft.value.group_line_start_mention_dispatch}
          onChange=${(e) => {
					updateDraft({ group_line_start_mention_dispatch: e.target.checked });
				}} />
        Dispatch on line-start mentions
      </label>
      <label class="text-xs text-[var(--muted)]" style="display:flex;align-items:center;gap:8px;margin-top:4px;">
        <input type="checkbox" data-field="groupReplyToDispatch" checked=${draft.value.group_reply_to_dispatch}
          onChange=${(e) => {
					updateDraft({ group_reply_to_dispatch: e.target.checked });
				}} />
        Dispatch on reply-to bot
      </label>
	      <label class="text-xs text-[var(--muted)]">Default Model</label>
	      <${ModelSelect} models=${modelsSig.value} value=${draft.value.model}
	        onChange=${(v) => {
					updateDraft({ model: v });
				}}
        placeholder=${defaultPlaceholder} />
      <label class="text-xs text-[var(--muted)]">DM Allowlist</label>
      <${AllowlistInput} value=${draft.value.allowlist} onChange=${(v) => {
				updateDraft({ allowlist: v });
			}} />
      ${error.value && html`<div class="text-xs text-[var(--error)] channel-error" style="display:block;">${error.value}</div>`}
      <button class="provider-btn"
        onClick=${onSubmit} disabled=${saving.value}>
        ${saving.value ? "Connecting\u2026" : "Connect Bot"}
      </button>
    </div>
  </${Modal}>`;
}

// ── Edit channel modal ───────────────────────────────────────
function EditChannelModal() {
	var ch = editingChannel.value;
	var draft = useSignal(ch ? editChannelDraftFromChannel(ch) : null);
	var error = useSignal("");
	var saving = useSignal(false);
	var requestVersion = useSignal(0);

	useEffect(() => {
		if (ch) {
			draft.value = editChannelDraftFromChannel(ch);
		} else {
			draft.value = null;
		}
		error.value = "";
		saving.value = false;
	}, [ch?.chanAccountKey]);

	function updateDraft(patch) {
		draft.value = { ...draft.value, ...patch };
	}

	function closeModal() {
		requestVersion.value += 1;
		draft.value = null;
		editingChannel.value = null;
		error.value = "";
		saving.value = false;
	}
	if (!ch) return null;
	var cfg = draft.value || editChannelDraftFromChannel(ch);

	function onSave(e) {
		e.preventDefault();
		error.value = "";
		saving.value = true;
		var requestId = requestVersion.value + 1;
		requestVersion.value = requestId;
		var modelFields = buildModelUpdateFields(cfg.model, ch.config || {});
		var updateConfig = {
			dm_policy: cfg.dm_policy,
			group_line_start_mention_dispatch: cfg.group_line_start_mention_dispatch,
			group_reply_to_dispatch: cfg.group_reply_to_dispatch,
			allowlist: cfg.allowlist.slice(),
			agent_id: cfg.agent_id ? cfg.agent_id : null,
			...modelFields,
		};
		sendRpc("channels.update", {
			chanAccountKey: ch.chanAccountKey,
			config: updateConfig,
		}).then((res) => {
			if (res?.ok) {
				loadChannels();
			}
			if (requestVersion.value !== requestId) return;
			saving.value = false;
			if (res?.ok) {
				closeModal();
			} else {
				error.value = (res?.error && (res.error.message || res.error.detail)) || "Failed to update bot.";
			}
		});
	}

	var defaultPlaceholder =
		modelsSig.value.length > 0
			? `(default: ${modelsSig.value[0].displayName || modelsSig.value[0].id})`
			: "(server default)";

	var selectStyle =
		"font-family:var(--font-body);background:var(--surface2);color:var(--text);border:1px solid var(--border);border-radius:4px;padding:8px 12px;font-size:.85rem;cursor:pointer;";

	var configuredAgent = cfg.agent_id || "";
	var agentMissing = Boolean(
		configuredAgent &&
			agentNamesLoaded.value &&
			Array.isArray(agentNames.value) &&
			!agentNames.value.includes(configuredAgent),
	);

	return html`<${Modal} show=${true} onClose=${closeModal} title="Edit Telegram Bot">
    <div class="channel-form">
			<div class="text-sm text-[var(--text-strong)]">${ch.name || ch.chanAccountKey}</div>
      <label class="text-xs text-[var(--muted)]">Agent (optional)</label>
				<div class="text-xs text-[var(--muted)]" style="margin-top:-2px;margin-bottom:6px;">
					Choose an agent under <code>${"agents/<agent_id>/"}</code>.
				</div>
      <select data-field="agentName" style=${selectStyle} value=${configuredAgent || ""} name="telegram_agent_name_edit"
        onChange=${(e) => {
					updateDraft({ agent_id: e.target.value });
				}}>
        <option value="">(default)</option>
        ${agentNames.value.map((id) => html`<option key=${id} value=${id}>${id}</option>`)}
        ${agentMissing ? html`<option value=${configuredAgent}>Missing: ${configuredAgent}</option>` : null}
      </select>
      <label class="text-xs text-[var(--muted)]">DM Policy</label>
      <select data-field="dmPolicy" style=${selectStyle} value=${cfg.dm_policy || "open"}
        onChange=${(e) => {
					updateDraft({ dm_policy: e.target.value });
				}}>
        <option value="open">Open (anyone)</option>
        <option value="allowlist">Allowlist only</option>
        <option value="disabled">Disabled</option>
      </select>
	      <label class="text-xs text-[var(--muted)]">Group Dispatch</label>
      <label class="text-xs text-[var(--muted)]" style="display:flex;align-items:center;gap:8px;margin-top:4px;">
        <input type="checkbox" data-field="groupLineStartMentionDispatch" checked=${cfg.group_line_start_mention_dispatch !== false}
          onChange=${(e) => {
					updateDraft({ group_line_start_mention_dispatch: e.target.checked });
				}} />
        Dispatch on line-start mentions
      </label>
      <label class="text-xs text-[var(--muted)]" style="display:flex;align-items:center;gap:8px;margin-top:4px;">
        <input type="checkbox" data-field="groupReplyToDispatch" checked=${cfg.group_reply_to_dispatch !== false}
          onChange=${(e) => {
					updateDraft({ group_reply_to_dispatch: e.target.checked });
				}} />
        Dispatch on reply-to bot
      </label>
	      <label class="text-xs text-[var(--muted)]">Default Model</label>
	      <${ModelSelect} models=${modelsSig.value} value=${cfg.model}
	        onChange=${(v) => {
					updateDraft({ model: v });
				}}
        placeholder=${defaultPlaceholder} />
      <label class="text-xs text-[var(--muted)]">DM Allowlist</label>
      <${AllowlistInput} value=${cfg.allowlist} onChange=${(v) => {
				updateDraft({ allowlist: v });
			}} />
      ${error.value && html`<div class="text-xs text-[var(--error)] channel-error" style="display:block;">${error.value}</div>`}
      <button class="provider-btn"
        onClick=${onSave} disabled=${saving.value}>
        ${saving.value ? "Saving\u2026" : "Save Changes"}
      </button>
    </div>
  </${Modal}>`;
}

// ── Main page component ──────────────────────────────────────
function ChannelsPage() {
	useEffect(() => {
		// Use prefetched cache for instant render
		if (S.cachedChannels !== null) channels.value = S.cachedChannels;
		if (connected.value) loadChannels();

		var unsub = onEvent("channel", (p) => {
			if (p.kind === "otp_resolved") {
				loadChannels();
			}
			var eventAccountHandle = p.chanAccountKey;
			if (
				activeTab.value === "senders" &&
				sendersAccount.value === eventAccountHandle &&
				(p.kind === "inbound_message" || p.kind === "otp_challenge" || p.kind === "otp_resolved")
			) {
				loadSenders();
			}
		});
		S.setChannelEventUnsub(unsub);

		return () => {
			if (unsub) unsub();
			S.setChannelEventUnsub(null);
		};
	}, [connected.value]);

	return html`
    <div class="flex-1 flex flex-col min-w-0 p-4 gap-4 overflow-y-auto">
      <div class="flex items-center gap-3">
        <h2 class="text-lg font-medium text-[var(--text-strong)]">Channels</h2>
        <div style="display:flex;gap:4px;margin-left:12px;">
          <button class="session-action-btn" style=${activeTab.value === "channels" ? "font-weight:600;" : ""}
            onClick=${() => {
							activeTab.value = "channels";
						}}>Channels</button>
          <button class="session-action-btn" style=${activeTab.value === "senders" ? "font-weight:600;" : ""}
            onClick=${() => {
							activeTab.value = "senders";
						}}>Senders</button>
        </div>
        ${
					activeTab.value === "channels" &&
					html`
          <button class="provider-btn"
            onClick=${() => {
							if (connected.value) showAddModal.value = true;
						}}>+ Add Telegram Bot</button>
        `
				}
      </div>
      ${activeTab.value === "channels" ? html`<${ChannelsTab} />` : html`<${SendersTab} />`}
    </div>
    <${AddChannelModal} />
    <${EditChannelModal} />
    <${ConfirmDialog} />
  `;
}

var _channelsContainer = null;

export function initChannels(container) {
	_channelsContainer = container;
	container.style.cssText = "flex-direction:column;padding:0;overflow:hidden;";
	activeTab.value = "channels";
	showAddModal.value = false;
	editingChannel.value = null;
	sendersAccount.value = "";
	senders.value = [];
	render(html`<${ChannelsPage} />`, container);
}

export function teardownChannels() {
	S.setRefreshChannelsPage(null);
	if (S.channelEventUnsub) {
		S.channelEventUnsub();
		S.setChannelEventUnsub(null);
	}
	if (_channelsContainer) render(null, _channelsContainer);
	_channelsContainer = null;
}
