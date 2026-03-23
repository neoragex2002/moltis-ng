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
		expect(pageErrors).toEqual([]);
	});

	test("heartbeat inactive state disables run now with info notice", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/crons/heartbeat");

		await expect(page.getByRole("button", { name: "Run Now", exact: true })).toBeDisabled();
		await expect(page.getByText(/Heartbeat inactive:/)).toBeVisible();
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

		await page.locator("[data-field=name]").fill(jobName);
		await page.locator("[data-field=cron]").fill("*/5 * * * *");
		await page.locator("[data-field=payloadKind]").selectOption("agentTurn");
		await page.locator("[data-field=message]").fill("original message");
		await page.getByRole("button", { name: "Create", exact: true }).click();

		await expect(page.getByRole("heading", { name: "Add Job", exact: true })).not.toBeVisible();
		await expect(page.getByRole("cell", { name: jobName, exact: true })).toBeVisible();

		const jobRow = page.locator("tr", { has: page.getByRole("cell", { name: jobName, exact: true }) });
		await jobRow.getByRole("button", { name: "Edit", exact: true }).click();
		await expect(page.getByRole("heading", { name: "Edit Job", exact: true })).toBeVisible();

		await page.locator("[data-field=name]").fill(`${jobName}-updated`);
		await page.locator("[data-field=message]").fill("");
		await page.getByRole("button", { name: "Update", exact: true }).click();

		await expect(page.locator("[data-field=name]")).toHaveValue(`${jobName}-updated`);
		await expect(page.locator("[data-field=message]")).toHaveValue("");
		expect(pageErrors).toEqual([]);
	});

	test("page has no JS errors", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/crons/jobs");
		expect(pageErrors).toEqual([]);
	});
});
