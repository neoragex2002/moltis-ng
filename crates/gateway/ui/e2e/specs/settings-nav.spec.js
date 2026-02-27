const { expect, test } = require("@playwright/test");
const { expectPageContentMounted, navigateAndWait, waitForWsConnected, watchPageErrors } = require("../helpers");

async function spoofSafari(page) {
	await page.addInitScript(() => {
		const safariUserAgent =
			"Mozilla/5.0 (Macintosh; Intel Mac OS X 14_3_1) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.3 Safari/605.1.15";
		Object.defineProperty(Navigator.prototype, "userAgent", {
			configurable: true,
			get() {
				return safariUserAgent;
			},
		});
		Object.defineProperty(Navigator.prototype, "vendor", {
			configurable: true,
			get() {
				return "Apple Computer, Inc.";
			},
		});
	});
}

test.describe("Settings navigation", () => {
	test("/settings redirects to /settings/personas", async ({ page }) => {
		await navigateAndWait(page, "/settings");
		await expect(page).toHaveURL(/\/settings\/personas$/);
		await expect(page.getByRole("heading", { name: "Personas", exact: true })).toBeVisible();
	});

	const settingsSections = [
		{ id: "personas", heading: "Personas" },
		{ id: "owner", heading: "Owner" },
		{ id: "memory", heading: "Memory" },
		{ id: "environment", heading: "Environment" },
		{ id: "crons", heading: "Cron Jobs" },
		{ id: "voice", heading: "Voice" },
		{ id: "security", heading: "Security" },
		{ id: "tailscale", heading: "Tailscale" },
		{ id: "notifications", heading: "Notifications" },
		{ id: "providers", heading: "LLMs" },
		{ id: "channels", heading: "Channels" },
		{ id: "mcp", heading: "MCP" },
		{ id: "hooks", heading: "Hooks" },
		{ id: "skills", heading: "Skills" },
		{ id: "sandboxes", heading: "Sandboxes" },
		{ id: "monitoring", heading: "Monitoring" },
		{ id: "logs", heading: "Logs" },
		{ id: "config", heading: "Configuration" },
	];

	for (const section of settingsSections) {
		test(`settings/${section.id} loads without errors`, async ({ page }) => {
			const pageErrors = watchPageErrors(page);
			await page.goto(`/settings/${section.id}`);
			await expectPageContentMounted(page);

			await expect(page).toHaveURL(new RegExp(`/settings/${section.id}$`));

			// Settings sections use heading text that may differ slightly
			// from the section ID; check the page loaded content.
			const content = page.locator("#pageContent");
			await expect(content).not.toBeEmpty();

			expect(pageErrors).toEqual([]);
		});
	}

		test("personas form elements render", async ({ page }) => {
			await navigateAndWait(page, "/settings/personas");
			await expect(page.getByRole("heading", { name: "Personas", exact: true })).toBeVisible();

			await expect(page.getByText("persona_id", { exact: true })).toBeVisible();
			await expect(page.locator("select")).toBeVisible();
			await expect(page.getByRole("button", { name: "Save", exact: true })).toBeVisible();
		});
	
	test("persona save shows Saved indicator", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/settings/personas");
		
		const nameInput = page.getByPlaceholder("e.g. Rex");
		await nameInput.fill("E2E Persona");
		await page.getByRole("button", { name: "Save", exact: true }).click();
		await expect(page.getByText("Saved", { exact: true })).toBeVisible();
		
		expect(pageErrors).toEqual([]);
	});

	test("selecting persona emoji does not crash", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/settings/personas");
		
		const pickBtn = page.getByRole("button", { name: "Pick", exact: true });
		await expect(pickBtn).toBeVisible();
		await pickBtn.click();

		const selectedEmoji = await page.evaluate(() => {
			var current = (window.__MOLTIS__?.identity?.emoji || "").trim();
			var options = ["🦊", "🐙", "🤖", "🐶"];
			return options.find((emoji) => emoji !== current) || "🦊";
		});
		await page.getByRole("button", { name: selectedEmoji, exact: true }).click();
		await page.getByRole("button", { name: "Save", exact: true }).click();
		await expect(page.getByText("Saved", { exact: true })).toBeVisible();

		expect(pageErrors).toEqual([]);
	});

	test("safari does not show favicon reload notice in personas", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await spoofSafari(page);
		await navigateAndWait(page, "/settings/personas");
		await expect(page.getByText("favicon updates requires reload", { exact: false })).toHaveCount(0);
		await expect(page.getByRole("button", { name: "requires reload", exact: true })).toHaveCount(0);
		expect(pageErrors).toEqual([]);
	});

	test("environment page has add form", async ({ page }) => {
		await navigateAndWait(page, "/settings/environment");
		await expect(page.getByRole("heading", { name: "Environment" })).toBeVisible();
		await expect(page.getByPlaceholder("KEY_NAME")).toHaveAttribute("autocomplete", "off");
		await expect(page.getByPlaceholder("Value")).toHaveAttribute("autocomplete", "new-password");
	});

	test("security page renders", async ({ page }) => {
		await navigateAndWait(page, "/settings/security");
		await expect(page.getByRole("heading", { name: "Security" })).toBeVisible();
	});

	test("provider page renders from settings", async ({ page }) => {
		await navigateAndWait(page, "/settings/providers");
		await expect(page.getByRole("heading", { name: "LLMs" })).toBeVisible();
	});

	test("channels add telegram token field is treated as a password", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/settings/channels");
		await waitForWsConnected(page);

		const addButton = page.getByRole("button", { name: "+ Add Telegram Bot", exact: true });
		await expect(addButton).toBeVisible();
		await addButton.click();

		await expect(page.getByRole("heading", { name: "Add Telegram Bot", exact: true })).toBeVisible();
		const tokenInput = page.getByPlaceholder("123456:ABC-DEF...");
		await expect(tokenInput).toHaveAttribute("type", "password");
		await expect(tokenInput).toHaveAttribute("autocomplete", "new-password");
		await expect(tokenInput).toHaveAttribute("name", "telegram_bot_token");
		expect(pageErrors).toEqual([]);
	});

	test("sidebar groups and order match product layout", async ({ page }) => {
		await navigateAndWait(page, "/settings/personas");

		await expect(page.locator(".settings-group-label").nth(0)).toHaveText("General");
		await expect(page.locator(".settings-group-label").nth(1)).toHaveText("Security");
		await expect(page.locator(".settings-group-label").nth(2)).toHaveText("Integrations");
		await expect(page.locator(".settings-group-label").nth(3)).toHaveText("Systems");

		const navItems = (await page.locator(".settings-nav-item").allTextContents()).map((text) => text.trim());
		const expectedWithVoice = [
			"Personas",
			"Owner",
			"Environment",
			"Memory",
			"Notifications",
			"Crons",
			"Security",
			"Tailscale",
			"Channels",
			"Hooks",
			"LLMs",
			"MCP",
			"Skills",
			"Voice",
			"Sandboxes",
			"Monitoring",
			"Logs",
			"Configuration",
		];
		const expectedWithoutVoice = expectedWithVoice.filter((item) => item !== "Voice");
		expect(navItems).toEqual(navItems.includes("Voice") ? expectedWithVoice : expectedWithoutVoice);

		const llmsNavItem = page.locator(".settings-nav-item", { hasText: "LLMs" });
		await expect(llmsNavItem.locator(".icon-layers")).toHaveCount(1);
		await expect(llmsNavItem.locator(".icon-server")).toHaveCount(0);

		const logsNavItem = page.locator(".settings-nav-item", { hasText: "Logs" });
		await expect(logsNavItem.locator(".icon-document")).toHaveCount(1);

		const configNavItem = page.locator(".settings-nav-item", { hasText: "Configuration" });
		await expect(configNavItem.locator(".icon-code")).toHaveCount(1);
		await expect(configNavItem.locator(".icon-document")).toHaveCount(0);
	});
});
