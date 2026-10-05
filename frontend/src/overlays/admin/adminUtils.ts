// admin 系の DOM 非依存 純関数（旧 app.js maskDiscordId）。

/** Discord ID を先頭4 + **** + 末尾4 でマスクする。6文字未満はそのまま。 */
export function maskDiscordId(discordId: string | null | undefined): string {
	if (!discordId || discordId.length < 6) return discordId ?? "";
	return `${discordId.substring(0, 4)}****${discordId.substring(discordId.length - 4)}`;
}
