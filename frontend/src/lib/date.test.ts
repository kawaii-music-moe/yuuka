import { afterEach, describe, expect, it, vi } from "vitest";
import { todayIso } from "./date";

const originalTimezone = process.env.TZ;
afterEach(() => {
	vi.useRealTimers();
	if (originalTimezone === undefined) delete process.env.TZ;
	else process.env.TZ = originalTimezone;
});

describe("todayIso", () => {
	it.each([
		["Asia/Tokyo", "2026-12-31T15:00:00Z", "2027-01-01"],
		["Asia/Tokyo", "2026-11-10T23:59:59Z", "2026-11-11"],
		["Asia/Tokyo", "2026-11-11T00:00:00Z", "2026-11-11"],
		["America/Los_Angeles", "2026-01-01T07:59:59Z", "2025-12-31"],
		["UTC", "2026-02-03T00:00:00Z", "2026-02-03"],
	])("uses local calendar in %s at %s", (timezone, instant, expected) => {
		process.env.TZ = timezone;
		vi.useFakeTimers();
		vi.setSystemTime(new Date(instant));
		expect(todayIso()).toBe(expected);
	});
});
