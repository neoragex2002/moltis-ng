const { expect, test } = require("@playwright/test");
const { navigateAndWait, watchPageErrors } = require("../helpers");

function installMockChannelsWs(page) {
	return page.addInitScript(() => {
		const channels = [
			{
				chanAccountKey: "telegram:123",
				name: "cute_alma_bot",
				type: "telegram",
				config: {
					agent_id: "agent-a",
					dm_policy: "open",
					group_line_start_mention_dispatch: true,
					group_reply_to_dispatch: true,
					allowlist: ["alice"],
					model: "custom-x",
					model_provider: "acme",
				},
			},
		];
		const models = [
			{ id: "model-1", displayName: "Model One", provider: "openai" },
			{ id: "model-2", displayName: "Model Two", provider: "anthropic" },
		];
		const agents = [{ name: "default" }, { name: "agent-a" }, { name: "agent-b" }];

		function clone(value) {
			return JSON.parse(JSON.stringify(value));
		}

		window.__mockChannelsRpc = {
			lastUpdateParams: null,
			lastAddParams: null,
			updateMode: "success",
			addMode: "success",
			updateDelayMs: 0,
			addDelayMs: 0,
			set(partial) {
				Object.assign(this, partial);
			},
			reset() {
				this.lastUpdateParams = null;
				this.lastAddParams = null;
			},
		};

		function mergeChannelConfig(target, patch) {
			for (const [key, value] of Object.entries(patch || {})) {
				target[key] = value;
			}
		}

		class MockWebSocket {
			static CONNECTING = 0;
			static OPEN = 1;
			static CLOSING = 2;
			static CLOSED = 3;

			constructor(url) {
				this.url = url;
				this.readyState = MockWebSocket.CONNECTING;
				this.onopen = null;
				this.onmessage = null;
				this.onclose = null;
				this.onerror = null;
				setTimeout(() => {
					if (this.readyState !== MockWebSocket.CONNECTING) return;
					this.readyState = MockWebSocket.OPEN;
					this.onopen && this.onopen({ target: this });
				}, 0);
			}

			send(raw) {
				const req = JSON.parse(raw);
				const respond = (frame, delayMs = 0) => {
					setTimeout(() => {
						if (this.readyState !== MockWebSocket.OPEN) return;
						this.onmessage && this.onmessage({ data: JSON.stringify(frame) });
					}, delayMs);
				};
				const ok = (payload, delayMs = 0) => respond({ type: "res", id: req.id, ok: true, payload }, delayMs);
				const fail = (message, delayMs = 0) =>
					respond({ type: "res", id: req.id, ok: false, error: { message } }, delayMs);

				switch (req.method) {
					case "connect":
						ok({ type: "hello-ok", server: { version: "test" } });
						return;
					case "models.list":
						ok(clone(models));
						return;
					case "channels.status":
						ok({ channels: clone(channels) });
						return;
					case "workspace.agent.list":
						ok({ agents: clone(agents) });
						return;
					case "logs.status":
						ok({ unseen_errors: 0, unseen_warns: 0 });
						return;
					case "sessions.list":
						ok([]);
						return;
					case "projects.list":
						ok([]);
						return;
					case "channels.update": {
						window.__mockChannelsRpc.lastUpdateParams = clone(req.params);
						const delayMs = window.__mockChannelsRpc.updateDelayMs || 0;
						if (window.__mockChannelsRpc.updateMode === "success") {
							mergeChannelConfig(channels[0].config, req.params?.config);
							ok({}, delayMs);
						} else {
							fail("forced update failure", delayMs);
						}
						return;
					}
					case "channels.add": {
						window.__mockChannelsRpc.lastAddParams = clone(req.params);
						const delayMs = window.__mockChannelsRpc.addDelayMs || 0;
						if (window.__mockChannelsRpc.addMode === "success") {
							ok({}, delayMs);
						} else {
							fail("forced add failure", delayMs);
						}
						return;
					}
					default:
						ok({});
				}
			}

			close() {
				this.readyState = MockWebSocket.CLOSED;
				this.onclose && this.onclose({ target: this });
			}

			addEventListener() {}
			removeEventListener() {}
		}

		window.WebSocket = MockWebSocket;
	});
}

async function openEditModal(page) {
	await page.getByRole("button", { name: "Edit", exact: true }).first().click();
	await expect(page.getByRole("heading", { name: "Edit Telegram Bot", exact: true })).toBeVisible();
}

async function openAddModal(page) {
	await page.getByRole("button", { name: "+ Add Telegram Bot", exact: true }).click();
	await expect(page.getByRole("heading", { name: "Add Telegram Bot", exact: true })).toBeVisible();
}

async function closeModal(page) {
	await page.locator(".modal-box button").filter({ hasText: "✕" }).click();
}

test.describe("Channels page", () => {
	test("edit telegram bot keeps draft and preserves model provider on unrelated save", async ({ page }) => {
		await installMockChannelsWs(page);
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/settings/channels");

		await page.evaluate(() => {
			window.__mockChannelsRpc.reset();
			window.__mockChannelsRpc.set({ updateMode: "fail", updateDelayMs: 0 });
		});
		await openEditModal(page);
		await page.locator("select[data-field=agentName]").selectOption("");
		await page.locator("select[data-field=dmPolicy]").selectOption("disabled");
		await page.getByRole("button", { name: "Save Changes", exact: true }).click();

		await expect(page.getByText("forced update failure")).toBeVisible();
		await expect(page.locator("select[data-field=agentName]")).toHaveValue("");
		await expect(page.locator("select[data-field=dmPolicy]")).toHaveValue("disabled");

		const payload = await page.evaluate(() => window.__mockChannelsRpc.lastUpdateParams);
		expect(payload.config.agent_id).toBeNull();
		expect(payload.config.dm_policy).toBe("disabled");
		expect(payload.config.model).toBe("custom-x");
		expect(payload.config.model_provider).toBe("acme");
		expect(Object.prototype.hasOwnProperty.call(payload.config, "token")).toBe(false);
		await closeModal(page);

		await page.evaluate(() => {
			window.__mockChannelsRpc.reset();
			window.__mockChannelsRpc.set({ updateMode: "fail", updateDelayMs: 200 });
		});
		await openEditModal(page);
		await page.locator("select[data-field=dmPolicy]").selectOption("allowlist");
		await page.getByRole("button", { name: "Save Changes", exact: true }).click();
		await closeModal(page);
		await page.waitForTimeout(350);
		await openEditModal(page);
		await expect(page.locator(".channel-error")).toHaveCount(0);
		await expect(page.locator("select[data-field=dmPolicy]")).toHaveValue("open");
		await closeModal(page);

		expect(pageErrors).toEqual([]);
	});

	test("add telegram bot keeps draft on failure and resets cleanly after reopen", async ({ page }) => {
		await installMockChannelsWs(page);
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/settings/channels");

		await page.evaluate(() => {
			window.__mockChannelsRpc.reset();
			window.__mockChannelsRpc.set({ addMode: "fail", addDelayMs: 0 });
		});
		await openAddModal(page);
		await page.locator("select[data-field=agentName]").selectOption("agent-b");
		await page.locator("input[data-field=token]").fill("123456:ABC");
		await page.locator("select[data-field=dmPolicy]").selectOption("disabled");
		await page.getByRole("button", { name: "Connect Bot", exact: true }).click();

		await expect(page.getByText("forced add failure")).toBeVisible();
		await expect(page.locator("select[data-field=agentName]")).toHaveValue("agent-b");
		await expect(page.locator("input[data-field=token]")).toHaveValue("123456:ABC");
		await expect(page.locator("select[data-field=dmPolicy]")).toHaveValue("disabled");

		const payload = await page.evaluate(() => window.__mockChannelsRpc.lastAddParams);
		expect(payload.config.agent_id).toBe("agent-b");
		expect(payload.config.token).toBe("123456:ABC");
		await closeModal(page);

		await openAddModal(page);
		await expect(page.locator("select[data-field=agentName]")).toHaveValue("");
		await expect(page.locator("input[data-field=token]")).toHaveValue("");
		await expect(page.locator("select[data-field=dmPolicy]")).toHaveValue("open");
		await expect(page.locator(".channel-error")).toHaveCount(0);
		await closeModal(page);

		await page.evaluate(() => {
			window.__mockChannelsRpc.reset();
			window.__mockChannelsRpc.set({ addMode: "fail", addDelayMs: 200 });
		});
		await openAddModal(page);
		await page.locator("input[data-field=token]").fill("999999:XYZ");
		await page.getByRole("button", { name: "Connect Bot", exact: true }).click();
		await closeModal(page);
		await page.waitForTimeout(350);
		await openAddModal(page);
		await expect(page.locator(".channel-error")).toHaveCount(0);
		await expect(page.locator("input[data-field=token]")).toHaveValue("");
		await closeModal(page);

		expect(pageErrors).toEqual([]);
	});
});
