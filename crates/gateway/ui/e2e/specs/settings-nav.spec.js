const { expect, test } = require("@playwright/test");
const { expectPageContentMounted, navigateAndWait, waitForWsConnected, watchPageErrors } = require("../helpers");

test.describe("Settings navigation", () => {
	test("/settings redirects to /settings/user", async ({ page }) => {
		await navigateAndWait(page, "/settings");
		await expect(page).toHaveURL(/\/settings\/user$/);
		await expect(page.getByRole("heading", { name: "User", exact: true })).toBeVisible();
	});

	const settingsSections = [
		{ id: "user" },
		{ id: "people" },
		{ id: "contacts" },
		{ id: "environment" },
		{ id: "memory" },
		{ id: "notifications" },
		{ id: "crons" },
		{ id: "security" },
		{ id: "tailscale" },
		{ id: "providers" },
		{ id: "channels" },
		{ id: "mcp" },
		{ id: "hooks" },
		{ id: "skills" },
		{ id: "voice" },
		{ id: "sandboxes" },
		{ id: "monitoring" },
		{ id: "logs" },
		{ id: "config" },
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

	test("user form elements render", async ({ page }) => {
		await navigateAndWait(page, "/settings/user");
		await expect(page.getByRole("heading", { name: "User", exact: true })).toBeVisible();

		await expect(page.getByText("USER.md", { exact: true })).toBeVisible();
		await expect(page.getByRole("button", { name: "Reload", exact: true })).toBeVisible();
		await expect(page.getByRole("button", { name: "Save", exact: true })).toBeVisible();
	});

	test("user save shows Saved indicator", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/settings/user");

		const ownerInput = page.locator("input.provider-key-input").first();
		await ownerInput.fill("E2E Owner");
		await page.getByRole("button", { name: "Save", exact: true }).click();
		await expect(page.getByText("Saved", { exact: true })).toBeVisible();

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
		await navigateAndWait(page, "/settings/user");

		await expect(page.locator(".settings-group-label").nth(0)).toHaveText("General");
		await expect(page.locator(".settings-group-label").nth(1)).toHaveText("Security");
		await expect(page.locator(".settings-group-label").nth(2)).toHaveText("Integrations");
		await expect(page.locator(".settings-group-label").nth(3)).toHaveText("Systems");

		const navItems = (await page.locator(".settings-nav-item").allTextContents()).map((text) => text.trim());
		const expectedWithVoice = [
			"User",
			"People",
			"Contacts",
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
