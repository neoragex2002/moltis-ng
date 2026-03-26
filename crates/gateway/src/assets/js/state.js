// ── Shared mutable state ────────────────────────────────────
import * as sig from "./signals.js";

export var ws = null;
export var reqId = 0;
export var connected = false;
export var reconnectDelay = 1000;
export var pending = {};
export var models = [];
export var activeProjectId = localStorage.getItem("moltis-project") || "";
export var projects = [];

// Chat-page specific DOM refs and input history
export var streamEl = null;
export var chatHistory = JSON.parse(localStorage.getItem("moltis-chat-history") || "[]");
export var chatHistoryIdx = -1;
export var chatHistoryDraft = "";

// Model selector elements — created dynamically inside the chat page
export var modelCombo = null;
export var modelComboBtn = null;
export var modelComboLabel = null;
export var modelDropdown = null;
export var modelSearchInput = null;
export var modelDropdownList = null;
export var selectedModelId = localStorage.getItem("moltis-model") || "";
export var modelIdx = -1;

// Session project combo (in chat header)
export var projectCombo = null;
export var projectComboBtn = null;
export var projectComboLabel = null;
export var projectDropdown = null;
export var projectDropdownList = null;

// Sandbox toggle
export var sandboxToggleBtn = null;
export var sandboxLabel = null;
export var sessionSandboxEnabled = true;
export var sessionSandboxImage = null;
export var sandboxImageBtn = null;
export var sandboxImageDropdown = null;
export var sandboxImageLabel = null;

// Chat page DOM refs
export var chatMsgBox = null;
export var chatInput = null;
export var chatSendBtn = null;
export var chatBatchLoading = false;

// Provider/channel page refresh callbacks
export var refreshProvidersPage = null;
export var refreshChannelsPage = null;
export var channelEventUnsub = null;

// Prefetched channel data
export var cachedChannels = null;
export function setCachedChannels(v) {
	cachedChannels = v;
	sig.cachedChannels.value = v;
}

// Sandbox
export var sandboxInfo = null;

// Logs
export var logsEventHandler = null;
export var unseenErrors = 0;
export var unseenWarns = 0;

// Project filter
export var projectFilterId = localStorage.getItem("moltis-project-filter") || "";

// DOM shorthand
export function $(id) {
	return document.getElementById(id);
}

// ── Setters ──────────────────────────────────────────────────
export function setWs(v) {
	ws = v;
}
export function setReqId(v) {
	reqId = v;
}
export function setConnected(v) {
	connected = v;
	sig.connected.value = v;
}
export function setReconnectDelay(v) {
	reconnectDelay = v;
}
export function setModels(v) {
	models = v;
	// Store signal is now owned by model-store.js; don't overwrite here.
}
export function setActiveProjectId(v) {
	activeProjectId = v;
}
export function setProjects(v) {
	projects = v;
	// Store signal is now owned by project-store.js; don't overwrite here.
}
export function setStreamEl(v) {
	streamEl = v;
}
export function setChatHistory(v) {
	chatHistory = v;
}
export function setChatHistoryIdx(v) {
	chatHistoryIdx = v;
}
export function setChatHistoryDraft(v) {
	chatHistoryDraft = v;
}
export function setModelCombo(v) {
	modelCombo = v;
}
export function setModelComboBtn(v) {
	modelComboBtn = v;
}
export function setModelComboLabel(v) {
	modelComboLabel = v;
}
export function setModelDropdown(v) {
	modelDropdown = v;
}
export function setModelSearchInput(v) {
	modelSearchInput = v;
}
export function setModelDropdownList(v) {
	modelDropdownList = v;
}
export function setSelectedModelId(v) {
	selectedModelId = v;
	// Store signal is now owned by model-store.js; don't overwrite here.
}
export function setModelIdx(v) {
	modelIdx = v;
}
export function setProjectCombo(v) {
	projectCombo = v;
}
export function setProjectComboBtn(v) {
	projectComboBtn = v;
}
export function setProjectComboLabel(v) {
	projectComboLabel = v;
}
export function setProjectDropdown(v) {
	projectDropdown = v;
}
export function setProjectDropdownList(v) {
	projectDropdownList = v;
}
export function setSandboxToggleBtn(v) {
	sandboxToggleBtn = v;
}
export function setSandboxLabel(v) {
	sandboxLabel = v;
}
export function setSessionSandboxEnabled(v) {
	sessionSandboxEnabled = v;
}
export function setSessionSandboxImage(v) {
	sessionSandboxImage = v;
}
export function setSandboxImageBtn(v) {
	sandboxImageBtn = v;
}
export function setSandboxImageDropdown(v) {
	sandboxImageDropdown = v;
}
export function setSandboxImageLabel(v) {
	sandboxImageLabel = v;
}
export function setChatMsgBox(v) {
	chatMsgBox = v;
}
export function setChatInput(v) {
	chatInput = v;
}
export function setChatSendBtn(v) {
	chatSendBtn = v;
}
export function setChatBatchLoading(v) {
	chatBatchLoading = v;
}
export function setRefreshProvidersPage(v) {
	refreshProvidersPage = v;
}
export function setRefreshChannelsPage(v) {
	refreshChannelsPage = v;
}
export function setChannelEventUnsub(v) {
	channelEventUnsub = v;
}
export function setLogsEventHandler(v) {
	logsEventHandler = v;
}
export function setUnseenErrors(v) {
	unseenErrors = v;
	sig.unseenErrors.value = v;
}
export function setUnseenWarns(v) {
	unseenWarns = v;
	sig.unseenWarns.value = v;
}
export function setProjectFilterId(v) {
	projectFilterId = v;
}
export function setSandboxInfo(v) {
	sandboxInfo = v;
	sig.sandboxInfo.value = v;
}
