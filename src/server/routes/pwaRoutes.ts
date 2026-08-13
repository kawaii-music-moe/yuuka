import {
	getContextNote,
	getContextNoteUpdatedAt,
	setContextNote,
} from "../../db/contextNoteRepo.js";
import {
	addExpense,
	getMonthlyIncomeTotal,
	getMonthlyTotal,
	listRecentExpenses,
} from "../../db/expenseRepo.js";
import { listPwaMessages } from "../../db/messageLogRepo.js";
import {
	createPersona,
	getActivePersonaIdForBot,
	getPersonaById,
	setActivePersonaForBot,
	updatePersona,
} from "../../db/personaRepo.js";
import { listSchedulesInRange } from "../../db/scheduleRepo.js";
import { addTodo, listTodos, updateTodo } from "../../db/todoRepo.js";
import {
	getUserGeminiConfig,
	getUserGoogleConfig,
	updateUserGeminiSettings,
} from "../../db/userRepo.js";
import { processMessage } from "../../gemini.js";
import {
	type RouteDef,
	type RouteRequestCtx,
	sendJson,
} from "../../types/contracts.js";

const BOT_ID = "system_default";
const DEFAULT_MODEL = "gemini-3.1-flash-lite";

function text(value: unknown): string {
	return typeof value === "string" ? value : "";
}

function requireUserId(ctx: RouteRequestCtx): string {
	if (!ctx.user) throw new Error("Authenticated user is required");
	return ctx.user.discordId;
}

function isoDate(value: string | null): string | undefined {
	return value ? new Date(value.replace(" ", "T")).toISOString() : undefined;
}

function messageReferences(content: string) {
	const normalized = content.toLowerCase();
	if (
		/(expense|income|budget|finance|payment|\u5bb6\u8a08|\u53ce\u652f|\u652f\u51fa|\u53ce\u5165)/.test(
			normalized,
		)
	) {
		return [
			{
				type: "finance" as const,
				title: "Finance",
				description: "Open the finance record",
				href: "/finance",
				meta: "Finance",
			},
		];
	}
	if (
		/(calendar|schedule|event|\u4e88\u5b9a|\u30ab\u30ec\u30f3\u30c0\u30fc)/.test(
			normalized,
		)
	) {
		return [
			{
				type: "calendar" as const,
				title: "Calendar",
				description: "Open the scheduled event",
				href: "/calendar",
				meta: "Calendar",
			},
		];
	}
	if (/(task|todo|to-do|\u30bf\u30b9\u30af)/.test(normalized)) {
		return [
			{
				type: "todo" as const,
				title: "Tasks",
				description: "Open the task list",
				href: "/todo",
				meta: "Tasks",
			},
		];
	}
	if (/(note|memory|\u30ce\u30fc\u30c8|\u30e1\u30e2)/.test(normalized)) {
		return [
			{
				type: "note" as const,
				title: "Shared note",
				description: "Open shared memory",
				href: "/notes",
				meta: "Note",
			},
		];
	}
	return undefined;
}

function getPersonaPrompt(userId: string): string {
	const id = getActivePersonaIdForBot(userId, BOT_ID);
	return id ? (getPersonaById(id)?.prompt ?? "") : "";
}

function savePersonaPrompt(userId: string, prompt: string): void {
	const personaId = getActivePersonaIdForBot(userId, BOT_ID);
	if (personaId) {
		updatePersona(userId, personaId, { prompt });
		return;
	}
	if (prompt.trim()) {
		const persona = createPersona(userId, "PWA persona", prompt);
		setActivePersonaForBot(userId, BOT_ID, persona.id);
	}
}

function mapTodo(item: ReturnType<typeof listTodos>[number]) {
	return {
		id: String(item.id),
		title: item.title,
		dueDate: item.due_date ?? undefined,
		completed: item.status === "done",
		list: "Personal",
	};
}

function mapTransaction(item: ReturnType<typeof listRecentExpenses>[number]) {
	return {
		id: String(item.id),
		date: item.date,
		category: item.category,
		description: item.memo ?? item.category,
		amount: item.amount,
		kind: item.type,
	};
}

export const pwaRoutes: RouteDef[] = [
	{
		method: "GET",
		path: "/api/pwa/status",
		auth: "user",
		async handler(ctx) {
			sendJson(ctx.res, 200, {
				status: "ok",
				service: "yuuka",
				checkedAt: new Date().toISOString(),
			});
		},
	},
	{
		method: "GET",
		path: "/api/pwa/settings",
		auth: "user",
		async handler(ctx) {
			const currentUserId = requireUserId(ctx);
			const ai = getUserGeminiConfig(currentUserId);
			const google = getUserGoogleConfig(currentUserId);
			sendJson(ctx.res, 200, {
				googleConnected: Boolean(google),
				googleAccount: google?.calendarId ?? undefined,
				model: ai?.model ?? DEFAULT_MODEL,
				maxTokens: 2048,
				temperature: 0.7,
				persona: getPersonaPrompt(currentUserId),
			});
		},
	},
	{
		method: "PUT",
		path: "/api/pwa/settings",
		auth: "user",
		async handler(ctx) {
			const userId = requireUserId(ctx);
			const current = getUserGeminiConfig(userId);
			const model = text(ctx.body.model) || current?.model || DEFAULT_MODEL;
			if (current)
				updateUserGeminiSettings(
					userId,
					current.apiKeyEncrypted,
					current.apiKeyIv,
					current.apiKeyTag,
					model,
				);
			if (typeof ctx.body.persona === "string")
				savePersonaPrompt(userId, ctx.body.persona);
			sendJson(ctx.res, 200, {
				googleConnected: Boolean(getUserGoogleConfig(userId)),
				model,
				maxTokens: Number(ctx.body.maxTokens) || 2048,
				temperature: Number(ctx.body.temperature) || 0.7,
				persona: getPersonaPrompt(userId),
			});
		},
	},
	{
		method: "GET",
		path: "/api/pwa/shared-note",
		auth: "user",
		async handler(ctx) {
			const userId = requireUserId(ctx);
			sendJson(ctx.res, 200, {
				id: "shared-note",
				title: "Shared note",
				body: getContextNote(userId, BOT_ID),
				updatedAt:
					getContextNoteUpdatedAt(userId, BOT_ID) ?? new Date().toISOString(),
			});
		},
	},
	{
		method: "PUT",
		path: "/api/pwa/shared-note",
		auth: "user",
		async handler(ctx) {
			const userId = requireUserId(ctx);
			const body = text(ctx.body.body);
			setContextNote(userId, BOT_ID, body);
			sendJson(ctx.res, 200, {
				id: "shared-note",
				title: text(ctx.body.title) || "Shared note",
				body,
				updatedAt: getContextNoteUpdatedAt(userId, BOT_ID),
			});
		},
	},
	{
		method: "GET",
		path: "/api/pwa/todos",
		auth: "user",
		async handler(ctx) {
			sendJson(
				ctx.res,
				200,
				listTodos(requireUserId(ctx), BOT_ID, { status: "all" }).map(mapTodo),
			);
		},
	},
	{
		method: "POST",
		path: "/api/pwa/todos",
		auth: "user",
		async handler(ctx) {
			const title = text(ctx.body.title).trim();
			if (!title)
				return sendJson(ctx.res, 400, { message: "title is required" });
			const todo = addTodo(requireUserId(ctx), BOT_ID, {
				title,
				dueDate: text(ctx.body.dueDate) || undefined,
			});
			sendJson(ctx.res, 201, {
				...mapTodo(todo),
				list: text(ctx.body.list) || "Personal",
			});
		},
	},
	{
		method: "PATCH",
		path: "/api/pwa/todos/:id",
		auth: "user",
		async handler(ctx) {
			const todo = updateTodo(
				requireUserId(ctx),
				BOT_ID,
				Number(ctx.params.id),
				{ status: ctx.body.completed === true ? "done" : "open" },
			);
			if (!todo) return sendJson(ctx.res, 404, { message: "Not found" });
			sendJson(ctx.res, 200, mapTodo(todo));
		},
	},
	{
		method: "GET",
		path: "/api/pwa/calendar/events",
		auth: "user",
		async handler(ctx) {
			const from = ctx.url.searchParams.get("from") || new Date().toISOString();
			const to =
				ctx.url.searchParams.get("to") ||
				new Date(Date.now() + 31 * 86400000).toISOString();
			const events = listSchedulesInRange(
				requireUserId(ctx),
				BOT_ID,
				from,
				to,
			).map((event) => ({
				id: String(event.id),
				title: event.title,
				startsAt: isoDate(event.start_at),
				endsAt: isoDate(event.end_at) ?? isoDate(event.start_at),
				calendar: event.google_calendar_id ?? "Personal",
				color: "#155eef",
			}));
			sendJson(ctx.res, 200, events);
		},
	},
	{
		method: "GET",
		path: "/api/pwa/finance/summary",
		auth: "user",
		async handler(ctx) {
			const [year, month] = (
				ctx.url.searchParams.get("month") ||
				new Date().toISOString().slice(0, 7)
			)
				.split("-")
				.map(Number);
			const userId = requireUserId(ctx);
			const income = getMonthlyIncomeTotal(userId, BOT_ID, year, month);
			const expense = getMonthlyTotal(userId, BOT_ID, year, month);
			sendJson(ctx.res, 200, {
				income,
				expense,
				balance: income - expense,
				month: `${year}-${String(month).padStart(2, "0")}`,
			});
		},
	},
	{
		method: "GET",
		path: "/api/pwa/finance/transactions",
		auth: "user",
		async handler(ctx) {
			sendJson(
				ctx.res,
				200,
				listRecentExpenses(requireUserId(ctx), BOT_ID, 100).map(mapTransaction),
			);
		},
	},
	{
		method: "POST",
		path: "/api/pwa/finance/transactions",
		auth: "user",
		async handler(ctx) {
			const amount = Number(ctx.body.amount);
			const category = text(ctx.body.category);
			if (!Number.isFinite(amount) || amount <= 0 || !category)
				return sendJson(ctx.res, 400, {
					message: "amount and category are required",
				});
			const item = addExpense(
				requireUserId(ctx),
				BOT_ID,
				amount,
				category,
				text(ctx.body.description) || undefined,
				text(ctx.body.date) || undefined,
				undefined,
				"pwa",
				ctx.body.kind === "income" ? "income" : "expense",
			);
			sendJson(ctx.res, 201, mapTransaction(item));
		},
	},
	{
		method: "GET",
		path: "/api/pwa/chat/messages",
		auth: "user",
		async handler(ctx) {
			const messages = listPwaMessages(requireUserId(ctx), BOT_ID).map(
				(message) => ({
					id: String(message.id),
					role: message.role === "assistant" ? "agent" : "user",
					content: message.content,
					createdAt: isoDate(message.created_at) ?? message.created_at,
					references:
						message.role === "assistant"
							? messageReferences(message.content)
							: undefined,
				}),
			);
			sendJson(ctx.res, 200, messages);
		},
	},
	{
		method: "POST",
		path: "/api/pwa/chat/messages",
		auth: "user",
		async handler(ctx) {
			const content = text(ctx.body.content).trim();
			if (!content)
				return sendJson(ctx.res, 400, { message: "content is required" });
			const result = await processMessage(BOT_ID, requireUserId(ctx), {
				text: content,
				source: "pwa",
			});
			sendJson(ctx.res, 201, {
				id: `pwa-${Date.now()}`,
				role: "agent",
				content: result.text,
				createdAt: new Date().toISOString(),
				references: messageReferences(result.text),
			});
		},
	},
];
