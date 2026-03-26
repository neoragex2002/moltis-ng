// ── Sandboxes page (runtime info + cached tool images) ──────────────────

import { signal } from "@preact/signals";
import { html } from "htm/preact";
import { render } from "preact";
import { useEffect } from "preact/hooks";
import { updateNavCount } from "./nav-counts.js";
import { sandboxInfo } from "./signals.js";

var images = signal([]);
var loading = signal(false);
var pruning = signal(false);

function fetchImages() {
	loading.value = true;
	fetch("/api/images/cached")
		.then((r) => (r.ok ? r.json() : { images: [] }))
		.then((data) => {
			images.value = data.images || [];
			updateNavCount("images", images.value.length);
		})
		.catch(() => {
			images.value = [];
		})
		.finally(() => {
			loading.value = false;
		});
}

function deleteImage(tag) {
	var encoded = encodeURIComponent(tag);
	fetch(`/api/images/cached/${encoded}`, { method: "DELETE" })
		.then((r) => {
			if (r.ok) fetchImages();
		})
		.catch(() => {
			/* ignore */
		});
}

function pruneAll() {
	pruning.value = true;
	fetch("/api/images/cached", { method: "DELETE" })
		.then((r) => {
			if (r.ok) fetchImages();
		})
		.catch(() => {
			/* ignore */
		})
		.finally(() => {
			pruning.value = false;
		});
}

function SandboxRuntimeCard() {
	var info = sandboxInfo.value;
	if (!info) return null;

	var backend = info.backend || "none";
	var os = info.os || "";
	var scopeKey = info.scopeKey || info.scope || "<n/a>";
	var idleTtlSecs = info.idleTtlSecs ?? info.idle_ttl_secs ?? 0;
	var image = info.image || "<unset>";
	var startupPolicy =
		info.startupContainerPolicy || info.startup_container_policy || "<n/a>";

	var status = backend === "none" ? "off" : "on";
	var badgeColor = backend === "none" ? "var(--error)" : "var(--muted)";

	return html`<div class="max-w-form">
    <h2 class="text-lg font-medium text-[var(--text-strong)]">Sandboxes</h2>
    <p class="text-sm text-[var(--muted)] leading-relaxed" style="margin:8px 0 12px;">
      Sandbox runtime is configured in <code>[tools.exec.sandbox]</code>. Moltis does not build or pull sandbox images.
    </p>
    <div class="info-bar" style="margin-bottom:8px;">
      <span class="info-field">
        <span class="info-label">Status:</span>
        <span class="info-value-strong" style="color:${badgeColor};font-family:var(--font-mono)">${status}</span>
      </span>
      <span class="info-field">
        <span class="info-label">Backend:</span>
        <span class="info-value-strong" style="font-family:var(--font-mono)">${backend}</span>
      </span>
      <span class="info-field">
        <span class="info-label">OS:</span>
        <span class="info-value-strong" style="font-family:var(--font-mono)">${os}</span>
      </span>
    </div>
    <div class="info-bar" style="margin-bottom:8px;">
      <span class="info-field">
        <span class="info-label">Scope key:</span>
        <span class="info-value-strong" style="font-family:var(--font-mono)">${scopeKey}</span>
      </span>
      <span class="info-field">
        <span class="info-label">Idle TTL:</span>
        <span class="info-value-strong" style="font-family:var(--font-mono)">${idleTtlSecs}s</span>
      </span>
      <span class="info-field">
        <span class="info-label">Startup policy:</span>
        <span class="info-value-strong" style="font-family:var(--font-mono)">${startupPolicy}</span>
      </span>
    </div>
    <div class="info-bar">
      <span class="info-field">
        <span class="info-label">Runtime image:</span>
        <span class="info-value-strong" style="font-family:var(--font-mono)">${image}</span>
      </span>
    </div>
  </div>`;
}

function ImageRow(props) {
	var img = props.image;
	return html`<div class="provider-item" style="margin-bottom:4px;">
    <div style="flex:1;min-width:0;">
      <div class="provider-item-name" style="font-family:var(--font-mono);font-size:.8rem;">${img.tag}</div>
      <div style="font-size:.7rem;color:var(--muted);margin-top:2px;display:flex;gap:12px;">
        <span>${img.size}</span>
        <span>${img.created}</span>
      </div>
    </div>
    <button class="session-action-btn session-delete" title="Delete cached image"
      onClick=${() => deleteImage(img.tag)}>x</button>
  </div>`;
}

function CachedImagesSection() {
	useEffect(() => {
		fetchImages();
	}, []);

	return html`<div class="max-w-form">
    <div class="flex items-center gap-3" style="margin-top:16px;">
      <h3 class="text-sm font-medium text-[var(--text-strong)]">Cached Tool Images</h3>
      <button class="text-xs text-[var(--muted)] border border-[var(--border)] px-2.5 py-1 rounded-md hover:text-[var(--text)] hover:border-[var(--border-strong)] transition-colors cursor-pointer bg-transparent"
			  onClick=${pruneAll} disabled=${pruning.value} title="Prune all cached tool images">
        ${pruning.value ? "Pruning\u2026" : "Prune all"}
      </button>
    </div>
    <p class="text-xs text-[var(--muted)] leading-relaxed" style="margin:8px 0 10px;">
      Docker images cached by Moltis for tool execution. This is separate from the sandbox runtime image.
    </p>
    ${loading.value && html`<div class="text-xs text-[var(--muted)]">Loading\u2026</div>`}
    ${!loading.value && images.value.length === 0 && html`<div class="text-xs text-[var(--muted)]" style="padding:12px 0;">No cached images.</div>`}
    ${images.value.map((img) => html`<${ImageRow} key=${img.tag} image=${img} />`)}
  </div>`;
}

function ImagesPage() {
	return html`
    <div class="flex-1 flex flex-col min-w-0 p-4 gap-4 overflow-y-auto">
      <${SandboxRuntimeCard} />
      <${CachedImagesSection} />
    </div>
  `;
}

var _imagesContainer = null;

export function initImages(container) {
	_imagesContainer = container;
	container.style.cssText = "flex-direction:column;padding:0;overflow:hidden;";
	images.value = [];
	render(html`<${ImagesPage} />`, container);
}

export function teardownImages() {
	if (_imagesContainer) render(null, _imagesContainer);
	_imagesContainer = null;
}

