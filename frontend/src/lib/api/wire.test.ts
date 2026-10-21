import { expect, expectTypeOf, it } from "vitest";
import { parseTaskTags } from "../../routes/tasks/taskUtils";
import type { PersonaRecord, TodoRecord, TodoWithSubtasks } from "./types";
import type { JsonWire } from "./wire";

it("maps JSON integer fields recursively without changing other wire fields", () => {
	expectTypeOf<
		JsonWire<{ id: bigint; children: { id: bigint | null }[] }>
	>().toEqualTypeOf<{ id: number; children: { id: number | null }[] }>();
	expectTypeOf<TodoRecord["tags"]>().toEqualTypeOf<string[]>();
	expectTypeOf<
		TodoWithSubtasks["subtasks"][number]["id"]
	>().toEqualTypeOf<number>();
	expectTypeOf<PersonaRecord["is_public"]>().toEqualTypeOf<boolean>();
});

it("shows tags from the Rust clean view and cached legacy JSON rows", () => {
	expect(parseTaskTags(["work", "today"])).toEqual(["work", "today"]);
	expect(parseTaskTags('["work"]')).toEqual(["work"]);
	expect(parseTaskTags("broken JSON")).toEqual([]);
});
