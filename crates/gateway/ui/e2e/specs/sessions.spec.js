const { expect, test } = require("@playwright/test");
const {
	expectPageContentMounted,
	navigateAndWait,
	waitForWsConnected,
	createSession,
	watchPageErrors,
	getActiveSessionId,
} = require("../helpers");

test.describe("Session management", () => {
	test("session list renders on load", async ({ page }) => {
		await navigateAndWait(page, "/");
		await waitForWsConnected(page);

		const sessionList = page.locator("#sessionList");
		await expect(sessionList).toBeVisible();

		// At least one session should be present
		const items = sessionList.locator(".session-item");
		await expect(items).not.toHaveCount(0);
	});

	test("stale browser session id falls back to service-owned home session", async ({ page }) => {
		const staleSessionId = "sess_stale_missing";
		await page.addInitScript((sessionId) => {
			localStorage.setItem("moltis-sessionId", sessionId);
		}, staleSessionId);

		const warnings = [];
		page.on("console", (msg) => {
			if (msg.type() === "warning") warnings.push(msg.text());
		});

		const pageErrors = await navigateAndWait(page, "/");
		await waitForWsConnected(page);

		await expect
			.poll(
				() =>
					page.evaluate((oldId) => {
						const store = window.__moltis_stores?.sessionStore;
						if (!store) return false;
						const activeSessionId = store.activeSessionId?.value || "";
						const activeSession = store.getById?.(activeSessionId);
						return (
							activeSessionId.length > 0 &&
							activeSessionId !== oldId &&
							(localStorage.getItem("moltis-sessionId") || "") === activeSessionId &&
							window.location.pathname === `/chats/${encodeURIComponent(activeSessionId)}` &&
							typeof activeSession?.displayName === "string" &&
							activeSession.displayName.trim().length > 0 &&
							activeSession?.sessionKind === "agent"
						);
					}, staleSessionId),
				{ timeout: 10_000 },
			)
			.toBe(true);

		expect(warnings.some((w) => w.includes('reason_code="stored_session_missing"'))).toBe(true);
		expect(pageErrors).toEqual([]);
	});

	test("sessions sidebar uses search and add button row", async ({ page }) => {
		const pageErrors = await navigateAndWait(page, "/");
		await waitForWsConnected(page);

		const sessionsPanel = page.locator("#sessionsPanel");
		await expect(sessionsPanel).toBeVisible();
		await expect(page.locator("#sessionSearch")).toBeVisible();
		await expect(page.locator("#newSessionBtn")).toBeVisible();

		const hasTopSessionsTitle = await page.evaluate(() => {
			const panel = document.getElementById("sessionsPanel");
			if (!panel) return false;
			const firstBlock = panel.firstElementChild;
			const title = firstBlock?.querySelector("span");
			return (title?.textContent || "").trim() === "Sessions";
		});
		expect(hasTopSessionsTitle).toBe(false);

		const searchAndAddShareRow = await page.evaluate(() => {
			const searchInput = document.getElementById("sessionSearch");
			const newSessionBtn = document.getElementById("newSessionBtn");
			if (!(searchInput && newSessionBtn)) return false;
			return searchInput.parentElement === newSessionBtn.parentElement;
		});
		expect(searchAndAddShareRow).toBe(true);

		expect(pageErrors).toEqual([]);
	});

	test("new session button creates a session", async ({ page }) => {
		const pageErrors = await navigateAndWait(page, "/");
		await waitForWsConnected(page);
		const homeSessionId = await getActiveSessionId(page);
		const sessionItems = page.locator("#sessionList .session-item");
		const initialCount = await sessionItems.count();

		await createSession(page);
		const firstSessionPath = new URL(page.url()).pathname;
		const firstSessionId = decodeURIComponent(firstSessionPath.replace(/^\/chats\//, ""));

		// URL should change to a new session (not home)
		expect(firstSessionId).not.toBe(homeSessionId);
		await expect(page).toHaveURL(/\/chats\//);
		await expect(page.locator(`#sessionList .session-item[data-session-id="${firstSessionId}"]`)).toHaveClass(
				/active/,
			);
		await expect(sessionItems).toHaveCount(initialCount + 1);
		await expect(page.locator("#chatInput")).toBeFocused();

		// Regression: creating a second session should still update the list
		// and mark the new session as active.
		await createSession(page);
		const secondSessionPath = new URL(page.url()).pathname;
		const secondSessionId = decodeURIComponent(secondSessionPath.replace(/^\/chats\//, ""));
		expect(secondSessionId).not.toBe(firstSessionId);
		await expect(page.locator(`#sessionList .session-item[data-session-id="${secondSessionId}"]`)).toHaveClass(
				/active/,
			);
		await expect(sessionItems).toHaveCount(initialCount + 2);
		await expect(page.locator("#chatInput")).toBeFocused();

		expect(pageErrors).toEqual([]);
	});

	test("new scratch sessions have distinct fallback display names", async ({ page }) => {
		const pageErrors = await navigateAndWait(page, "/");
		await waitForWsConnected(page);

		await createSession(page);
		await createSession(page);

		const labels = await page.locator("#sessionList .session-item [data-label-text]").allTextContents();
		const chatLabels = labels.filter((label) => /^Chat /.test(label));

		expect(chatLabels.length).toBeGreaterThanOrEqual(2);
		expect(new Set(chatLabels).size).toBe(chatLabels.length);
		expect(pageErrors).toEqual([]);
	});

	test("clicking a session switches to it", async ({ page }) => {
		await navigateAndWait(page, "/");
		await waitForWsConnected(page);

		// Create a second session so we have something to switch to
		await createSession(page);
		const newSessionUrl = page.url();

		const inactiveSessionItem = page.locator("#sessionList .session-item:not(.active)").first();
		await expect(inactiveSessionItem).toBeVisible();
		await inactiveSessionItem.click();

		await expect(page).not.toHaveURL(newSessionUrl);
		await expectPageContentMounted(page);
	});

	test("home session shows clear action while scratch sessions show delete", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await page.goto("/");
		await waitForWsConnected(page);
		await expectPageContentMounted(page);

		await expect(page.locator('button[title="Clear session"]')).toBeVisible();
		await expect(page.locator('button[title="Delete session"]')).toHaveCount(0);

		await createSession(page);

		await expect(page.locator('button[title="Clear session"]')).toHaveCount(0);
		await expect(page.locator('button[title="Delete session"]')).toBeVisible();

		expect(pageErrors).toEqual([]);
	});

	test("main session preview updates after clear on first message without reload", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/");
		await waitForWsConnected(page);
		const homeSessionId = await getActiveSessionId(page);

		const chatInput = page.locator("#chatInput");
		await expect(chatInput).toBeVisible();
		await expect(chatInput).toBeEnabled();

		await chatInput.fill("/clear");
		await chatInput.press("Enter");

		await expect
			.poll(
				() =>
					page.evaluate(() => {
						const store = window.__moltis_stores?.sessionStore;
						const activeSessionId = store?.activeSessionId?.value || "";
						const active = store?.getById?.(activeSessionId);
						if (!active) return null;
						return {
							messageCount: active.messageCount || 0,
							preview: active.preview || "",
						};
					}),
				{ timeout: 10_000 },
			)
			.toEqual({ messageCount: 0, preview: "" });

		const firstMessage = "sidebar preview should update immediately";
		await chatInput.fill(firstMessage);
		await chatInput.press("Enter");

		await expect(
			page.locator(`#sessionList .session-item[data-session-id="${homeSessionId}"] .session-preview`),
		).toContainText(
				firstMessage,
			);

		expect(pageErrors).toEqual([]);
	});

	test("session search filters the list", async ({ page }) => {
		await navigateAndWait(page, "/");
		await waitForWsConnected(page);

		const searchInput = page.locator("#sessionSearch");
		// searchInput may be hidden until focused or may always be visible
		if (await searchInput.isVisible()) {
			const countBefore = await page.locator("#sessionList .session-item").count();

			// Type a string that won't match any session
			await searchInput.fill("zzz_no_match_zzz");
			// Allow time for filtering
			await page.waitForTimeout(300);

			const countAfter = await page.locator("#sessionList .session-item").count();
			expect(countAfter).toBeLessThanOrEqual(countBefore);

			// Clear search restores list
			await searchInput.fill("");
			await page.waitForTimeout(300);

			const countRestored = await page.locator("#sessionList .session-item").count();
			expect(countRestored).toBe(countBefore);
		}
	});

	test("pending session item shows Loading… placeholder (no sessionId flash)", async ({ page }) => {
		const pageErrors = await navigateAndWait(page, "/");
		await waitForWsConnected(page);

		const pendingId = "sess_pending_test";
		await page.evaluate((sessionId) => {
			const store = window.__moltis_stores?.sessionStore;
			store?.upsert?.({ sessionId, clientOnly: true, displayName: "", sessionKind: "agent", canDelete: true });
		}, pendingId);

		await expect(
			page.locator(`#sessionList .session-item[data-session-id="${pendingId}"] [data-label-text]`),
		).toHaveText("Loading…");

		expect(pageErrors).toEqual([]);
	});

	test("invalid session item shows Invalid session placeholder and logs warning", async ({ page }) => {
		const warnings = [];
		page.on("console", (msg) => {
			if (msg.type() === "warning") warnings.push(msg.text());
		});

		const pageErrors = await navigateAndWait(page, "/");
		await waitForWsConnected(page);

		const invalidId = "sess_invalid_test";
		await page.evaluate((sessionId) => {
			const store = window.__moltis_stores?.sessionStore;
			store?.upsert?.({ sessionId, clientOnly: false, displayName: "", sessionKind: "agent", canDelete: true });
		}, invalidId);

		await expect(
			page.locator(`#sessionList .session-item[data-session-id="${invalidId}"] [data-label-text]`),
		).toHaveText("Invalid session");
		expect(warnings.some((w) => w.includes('reason_code="missing_display_name"'))).toBe(true);

		expect(pageErrors).toEqual([]);
	});

	test("clear all sessions resets list", async ({ page }) => {
		await navigateAndWait(page, "/");
		await waitForWsConnected(page);

		// Create extra sessions first
		await createSession(page);
		await createSession(page);

		const clearBtn = page.locator("#clearAllSessionsBtn");
		if (await clearBtn.isVisible()) {
			await expect(clearBtn).toHaveText("Clear All");
			// Accept the confirm dialog
			page.on("dialog", (dialog) => dialog.accept());
			await clearBtn.click();

			// Wait for list to reset
			await page.waitForTimeout(500);
			await expectPageContentMounted(page);

			// Should be back to a single session
			const items = page.locator("#sessionList .session-item");
			const count = await items.count();
			expect(count).toBeGreaterThanOrEqual(1);
		}
	});

	test("sessions panel hidden on non-chat pages", async ({ page }) => {
		await navigateAndWait(page, "/settings");

		const panel = page.locator("#sessionsPanel");
		// On settings pages, the sessions panel should be hidden
		await expect(panel).toBeHidden();
	});

	test("deleting unmodified fork skips confirmation dialog", async ({ page }) => {
		const pageErrors = await navigateAndWait(page, "/");
		await waitForWsConnected(page);

		// Create a session so we're not on "main" (Delete button is hidden for main)
		await createSession(page);
		const sessionUrl = page.url();

		// Simulate an unmodified fork: set forkPoint = messageCount = 5
		// so the session looks like a fork with messages but no new ones added.
		await expect
			.poll(
				() =>
					page.evaluate(() => {
						const store = window.__moltis_stores?.sessionStore;
						const session = store?.activeSession?.value;
						if (!session) return false;
						session.forkPoint = 5;
						session.messageCount = 5;
						// Bump dataVersion to trigger re-render
						session.dataVersion.value++;
						return true;
					}),
				{ timeout: 10_000 },
			)
			.toBe(true);

		// Click the Delete button — should NOT show a confirmation dialog
		const deleteBtn = page.locator('button[title="Delete session"]');
		await expect(deleteBtn).toBeVisible();
		await deleteBtn.click();

		// The session should be deleted immediately (no dialog appeared)
		// so we should navigate away from the current session URL
		await page.waitForURL((url) => url.href !== sessionUrl, { timeout: 5_000 });
		await expectPageContentMounted(page);

		// The confirmation dialog should NOT be visible
		await expect(page.locator(".provider-modal-backdrop")).toHaveCount(0);

		expect(pageErrors).toEqual([]);
	});

	test("deleting modified fork still shows confirmation dialog", async ({ page }) => {
		const pageErrors = await navigateAndWait(page, "/");
		await waitForWsConnected(page);

		await createSession(page);
		const activeSessionId = await getActiveSessionId(page);
		await expect
			.poll(
				() =>
					page.evaluate(() => {
						const store = window.__moltis_stores?.sessionStore;
						const session = store?.activeSession?.value;
						if (!store || !session) return false;
						return store.switchInProgress?.value === false && session.clientOnly === false;
					}),
				{ timeout: 10_000 },
			)
			.toBe(true);

		// Simulate a modified fork: messageCount > forkPoint
		await expect
			.poll(
				() =>
					page.evaluate((sessionId) => {
						const store = window.__moltis_stores?.sessionStore;
						const session = store?.getById?.(sessionId) || store?.activeSession?.value;
						if (!session) return null;
						session.forkPoint = 3;
						session.messageCount = 5;
						session.dataVersion.value++;
						store?.notify?.();
						return { forkPoint: session.forkPoint, messageCount: session.messageCount };
					}, activeSessionId),
				{ timeout: 10_000 },
			)
			.toEqual({ forkPoint: 3, messageCount: 5 });

		const deleteBtn = page.locator('button[title="Delete session"]');
		await expect(deleteBtn).toBeVisible();
		await deleteBtn.click();

		// The confirmation dialog SHOULD appear
		await expect(page.locator(".provider-modal-backdrop")).toBeVisible();

		// Dismiss the dialog by clicking Cancel
		await page.locator(".provider-modal-backdrop .provider-btn-secondary").click();
		await expect(page.locator(".provider-modal-backdrop")).toHaveCount(0);

		expect(pageErrors).toEqual([]);
	});
});
