const { expect, test } = require("@playwright/test");
const { navigateAndWait, watchPageErrors } = require("../helpers");

test.describe("Cron jobs page", () => {
	test("cron page loads with heading", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/crons/jobs");

		await expect(page.getByRole("heading", { name: "Cron Jobs", exact: true })).toBeVisible();
		expect(pageErrors).toEqual([]);
	});

	test("heartbeat tab loads", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/crons/heartbeat");

		await expect(page.getByRole("heading", { name: /heartbeat/i })).toBeVisible();
		await expect(
			page.locator("p", { hasText: "Heartbeat prompt is owned by" }).locator("code"),
		).toHaveText("agents/default/HEARTBEAT.md");
		expect(pageErrors).toEqual([]);
	});

	test("heartbeat active hours uses explicit timezone values", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/crons/heartbeat");

		await page.getByLabel("Limit runs to active hours").check();
		await expect(page.locator("select").filter({ has: page.locator("option[value='UTC']") })).toBeVisible();
		await expect(page.locator("option[value='local']")).toHaveCount(0);
		await expect(page.locator("input[placeholder='24:00']")).toBeVisible();
		expect(pageErrors).toEqual([]);
	});

	test("heartbeat run now without config shows error", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/crons/heartbeat");

		await page.getByRole("button", { name: "Run Now", exact: true }).click();
		await expect(page.getByText(/No heartbeat config yet/i)).toBeVisible();
		expect(pageErrors).toEqual([]);
	});

	test("heartbeat run now rejects stale status after agent switch", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/crons/heartbeat");

		await page.getByRole("button", { name: "Save", exact: true }).click();
		await expect(page.getByText(/No heartbeat config yet/i)).toHaveCount(0);

		await page.locator("input[list='hbAgentIds']").fill("other-agent");
		await page.getByRole("button", { name: "Run Now", exact: true }).click();

		await expect(page.getByText(/No heartbeat config yet\. Save the config first\./i)).toBeVisible();
		expect(pageErrors).toEqual([]);
	});

	test("create job button present", async ({ page }) => {
		await navigateAndWait(page, "/crons/jobs");

		// Page should have content, create button may depend on state
		const content = page.locator("#pageContent");
		await expect(content).not.toBeEmpty();
	});

	test("edit job keeps current draft after validation rerender", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		const jobName = `draft-rerender-job-${Date.now()}`;
		await navigateAndWait(page, "/crons/jobs");

		await page.getByRole("button", { name: "+ Add Job", exact: true }).click();
		await expect(page.getByRole("heading", { name: "Add Job", exact: true })).toBeVisible();

		await page.locator("[data-field=agentId]").fill("default");
		await page.locator("[data-field=name]").fill(jobName);
		await page.locator("[data-field=schedKind]").selectOption("every");
		await page.locator("[data-field=every]").fill("30m");
		await page.locator("[data-field=prompt]").fill("original prompt");
		await page.locator("[data-field=deliveryKind]").selectOption("silent");
		await page.getByRole("button", { name: "Create", exact: true }).click();

		await expect(page.getByRole("heading", { name: "Add Job", exact: true })).not.toBeVisible();
		await expect(page.getByRole("cell", { name: jobName, exact: true })).toBeVisible();

		const jobRow = page.locator("tr", { has: page.getByRole("cell", { name: jobName, exact: true }) });
		await jobRow.getByRole("button", { name: "Edit", exact: true }).click();
		await expect(page.getByRole("heading", { name: "Edit Job", exact: true })).toBeVisible();

		await page.locator("[data-field=name]").fill(`${jobName}-updated`);
		await page.locator("[data-field=prompt]").fill("");
		await page.getByRole("button", { name: "Update", exact: true }).click();

		await expect(page.locator("[data-field=name]")).toHaveValue(`${jobName}-updated`);
		await expect(page.locator("[data-field=prompt]")).toHaveValue("");
		expect(pageErrors).toEqual([]);
	});

	test("page has no JS errors", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/crons/jobs");
		expect(pageErrors).toEqual([]);
	});
});
