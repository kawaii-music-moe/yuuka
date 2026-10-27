import { describe, expect, it } from "vitest";
import { pickerKeyAction } from "./datePicker";

describe("pickerKeyAction", () => {
	it("blocks typed characters", () => {
		for (const key of ["1", "a", "/", "-", "ArrowLeft", "ArrowRight"]) {
			expect(pickerKeyAction(key)).toBe("block");
		}
	});

	it("opens the picker with space and up/down", () => {
		for (const key of [" ", "ArrowDown", "ArrowUp"]) {
			expect(pickerKeyAction(key)).toBe("open");
		}
	});

	it("clears with Backspace / Delete", () => {
		expect(pickerKeyAction("Backspace")).toBe("clear");
		expect(pickerKeyAction("Delete")).toBe("clear");
	});

	it("passes focus moves, submit and shortcuts through", () => {
		expect(pickerKeyAction("Tab")).toBe("pass");
		expect(pickerKeyAction("Enter")).toBe("pass");
		expect(pickerKeyAction("Escape")).toBe("pass");
		expect(pickerKeyAction("c", { ctrlKey: true })).toBe("pass");
		expect(pickerKeyAction("v", { metaKey: true })).toBe("pass");
	});
});
