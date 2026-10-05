// 家計簿タブの純関数（DOM 非依存）。旧 app.js の集計・整形ロジックを切り出したもの。
import type { BudgetLimit, CategoryTotal, ExpenseRecord } from "$lib/api/types";

/** 家計簿カテゴリ選択肢（旧 index.html の <select> と共通）。 */
export const EXPENSE_CATEGORIES = [
	"食費",
	"日用品",
	"交通費",
	"光熱費",
	"通信費",
	"医療費",
	"娯楽",
	"衣服",
	"その他",
] as const;

/** ローカルの今日 'YYYY-MM-DD'（旧 new Date().toISOString().slice(0,10)）。 */
export function todayIso(): string {
	return new Date().toISOString().slice(0, 10);
}

/** 現在時刻 'HH:mm:ss'（旧 expenseForm submit の time 生成）。 */
export function nowTime(): string {
	const now = new Date();
	const pad = (n: number) => n.toString().padStart(2, "0");
	return `${pad(now.getHours())}:${pad(now.getMinutes())}:${pad(now.getSeconds())}`;
}

/** 金額を '¥1,234' へ。 */
export function yen(amount: number): string {
	return `¥${amount.toLocaleString()}`;
}

export interface BudgetBar {
	category: string;
	spent: number;
	limit: number;
	/** 0..100 にクランプ済みの割合 */
	pct: number;
	/** バー塗り色（CSS 変数 or 固定色） */
	color: string;
}

/**
 * カテゴリ別予算進捗バーのデータを組み立てる（旧 renderCategoryBudgetBars）。
 * 上限設定済みカテゴリのみを対象に、breakdown の支出を突き合わせる。
 */
export function buildBudgetBars(
	breakdown: CategoryTotal[],
	limits: BudgetLimit[],
): BudgetBar[] {
	const spendMap = new Map<string, number>();
	for (const b of breakdown) spendMap.set(b.category, b.total);

	return limits.map((lim) => {
		const spent = spendMap.get(lim.category) ?? 0;
		const pct = Math.min((spent / lim.limit_amount) * 100, 100);
		const color =
			pct > 90
				? "var(--color-red)"
				: pct > 60
					? "#fbbf24"
					: "var(--color-primary)";
		return {
			category: lim.category,
			spent,
			limit: lim.limit_amount,
			pct,
			color,
		};
	});
}

/** 収支レコードの登録元表示（旧 fetchExpensesList の source 判定）。 */
export type ExpenseSourceKey = "receipt" | "plan" | "manual";

export function expenseSource(exp: ExpenseRecord): {
	key: ExpenseSourceKey;
	icon: string;
	label: string;
} {
	const key: ExpenseSourceKey =
		exp.source === "receipt" || exp.source === "receipt_ocr"
			? "receipt"
			: exp.source === "plan"
				? "plan"
				: "manual";
	const icon =
		key === "receipt"
			? "photo_camera"
			: key === "plan"
				? "event_available"
				: "web";
	const label =
		key === "receipt" ? "レシートAI" : key === "plan" ? "支払い予定" : "手動";
	return { key, icon, label };
}

/** 日付＋任意の時刻を表示（旧 exp.time ? `${date} ${time}` : date）。 */
export function formatExpenseDate(exp: ExpenseRecord): string {
	return exp.time ? `${exp.date} ${exp.time.substring(0, 5)}` : exp.date;
}

/**
 * レシート画像 File を base64（data URI のペイロード部）＋ MIME へ変換する
 * （旧 handleReceiptScan の FileReader.readAsDataURL → result.split(",")[1]）。
 * サーバは JSON { imageBase64, mimeType } を要求する（multipart 非対応）。
 */
export function readReceiptFile(
	file: File,
): Promise<{ imageBase64: string; mimeType: string }> {
	return new Promise((resolve, reject) => {
		const reader = new FileReader();
		reader.onload = () => {
			const result = reader.result;
			if (typeof result !== "string") {
				reject(new Error("画像の読み込みに失敗しました。"));
				return;
			}
			resolve({ imageBase64: result.split(",")[1] ?? "", mimeType: file.type });
		};
		reader.onerror = () => reject(new Error("画像の読み込みに失敗しました。"));
		reader.readAsDataURL(file);
	});
}
