import { describe, expect, it } from "vitest";
import { formatDeviceCode, formatElapsed } from "./deviceUtils";

describe("formatDeviceCode", () => {
	it("XXXX-XXXX に整形する", () => {
		expect(formatDeviceCode("wdjbmjht")).toBe("WDJB-MJHT");
		expect(formatDeviceCode("wd")).toBe("WD");
	});
});

describe("formatElapsed", () => {
	it("60秒未満は秒、以降は分で表す", () => {
		expect(formatElapsed(0)).toBe("0秒前");
		expect(formatElapsed(59)).toBe("59秒前");
		expect(formatElapsed(60)).toBe("1分前");
		expect(formatElapsed(-3)).toBe("0秒前");
	});
});
