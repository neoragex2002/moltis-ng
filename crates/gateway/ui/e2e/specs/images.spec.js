const { expect, test } = require("@playwright/test");
const { navigateAndWait, watchPageErrors } = require("../helpers");

test.describe("Images page (sandbox runtime + cached tool images)", () => {
	test("loads sandbox runtime card without build controls", async ({ page }) => {
		const pageErrors = watchPageErrors(page);
		await navigateAndWait(page, "/settings/sandboxes");

		await expect(
			page.getByRole("heading", { name: "Sandboxes", exact: true }),
		).toBeVisible();
		await expect(
			page.getByText(/Moltis does not build or pull sandbox images/i),
		).toBeVisible();

		await expect(page.getByRole("button", { name: /build/i })).toHaveCount(0);
		expect(pageErrors).toEqual([]);
	});
});
