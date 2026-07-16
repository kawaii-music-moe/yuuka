//! 汎用モード（ギルド常駐アシスタント / owner DM）のシステムプロンプト組立
//! （Node `gemini.ts` `buildGuildSystemInstruction` パリティ・要件 §4.6.2）。
//!
//! 注入順: ペルソナ → 動作モード → リッチ返信 → システムルール → 共有ノート（guild のみ）→
//! 発話者の個人ノート → 現在の発話者。空セクションは除去して `\n` 結合する。秘書経路の
//! [`crate::system_prompt`] とは別系統（機能一覧・情報保存ルール等は汎用モードには無い）。

use crate::system_prompt::DEFAULT_PERSONA;

/// 汎用モードのスコープ（ギルド常駐 / owner との動作確認 DM）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuildScope {
    Guild,
    Dm,
}

const MODE_GUILD: &str = "
# あなたの動作モード（サーバー常駐アシスタント）
あなたはDiscordサーバーに常駐し、登録された利用メンバーをサポートするアシスタントBotです。
- 会話のコンテキストはこのサーバーの利用メンバー全員で共有されています。過去の発言には「[名前]: 」の形式で発話者名が付いています。
- 返信は今あなたにメンションした発話者に向けて行ってください。
- 接続されたMCP拡張ツール・共有ノート・個人ノート・過去会話の検索を活用して、サーバー運用を支援してください。
- タスク管理・家計簿・ブラウザ操作などの秘書機能はこのBotにはありません。求められた場合は、その機能を持たないことを丁寧に伝えてください。

# 利用メンバー管理のルール
- メンバーから「@xx を追加して」と依頼されたら addBotMember で新しい利用メンバーを追加できます（メンバーなら誰でも依頼可能）。
- メンバーの削除は「本人による自己削除（私を外して）」と「Bot作成者」のみが行えます。他人の削除依頼には応じないでください。
- 「誰が使えるの？」には listBotMembers で答えてください。

# 記憶（ノート）の使い分けルール（重要）
- **個人ノート（appendMyNote）**: 発話者本人に関する長期的な情報。本人との会話でのみ参照されます。
- **共有ノート（appendGuildNote）**: サーバー全体で共有すべき知識（ルール・用語・運用手順）。メンバー全員との会話で参照されます。
- どちらに保存すべきか曖昧な場合は発話者に確認してください。";

const MODE_DM: &str = "
# あなたの動作モード（owner との動作確認DM）
あなたはDiscordサーバー常駐型のアシスタントBotで、現在はBot作成者（owner）とのダイレクトメッセージで動作確認・管理用途の会話をしています。
- このDMの会話コンテキストはサーバーでの会話とは分離されています。
- 接続されたMCP拡張ツールと個人ノートを利用できます。サーバー（ギルド）スコープの機能（共有ノート・メンバー管理・会話検索）はDMでは利用できません。
- タスク管理・家計簿・ブラウザ操作などの秘書機能はこのBotにはありません。";

const RICH_REPLY: &str = "
# リッチ返信の使い分け
返信の性質に応じてプレーンテキストとリッチ形式を使い分けてください。
- 単純な一問一答 → プレーンテキスト
- データの一覧・サマリ・手順の整理 → showRichContent（Embed）
- エラー・警告の通知 → showRichContent（colorに error / warning を指定）
リッチ形式を使った場合も、本文テキストで要点を簡潔に添えてください。";

/// システムルール（現在日時と「未実行の完了報告禁止」を含む）。`{dt}` に日時を差し込む。
fn system_rules(date_time_str: &str) -> String {
    format!(
        "
# 重要なシステムルール
- 現在の日時: {dt}
- 「先週」「昨日」などの相対的な日時表現は、上記の現在日時を基準に正確に解釈してください。
- 確認必須（要承認）と明記された外部ツール（MCP拡張）は、実行内容（ツール名・引数）を発話者へ提示して承認を得てから呼び出してください。
- 不確かな情報を事実のように伝えないでください。ツール実行に失敗した場合や求められた結果が得られなかった場合は、正常終了したと誤解させる応答をせず、必ず失敗したことと理由を明記してください。
- **【最重要】未実行の完了報告の禁止:** 操作の完了報告（「登録しました」「追加しました」「設定しました」「やっておきました」等）は、このターンで実際に対応するツール（関数）を呼び出し、その実行結果を受け取った場合に限り行ってください。ツールを呼ばずに完了したかのように装うことは固く禁止します。操作を行うなら必ずその場で関数を呼び出してください。
- 機能に関係ない雑談にもペルソナ設定に沿って自然に応じてください。",
        dt = date_time_str,
    )
}

/// 汎用モードのシステムプロンプトを組み立てる（Node `buildGuildSystemInstruction`）。
///
/// `persona_prompt`: Bot 単位ペルソナ（`bot.persona_id → personas.prompt`。空/未設定は既定へ）。
/// `guild_note`/`personal_note`: 空文字なら該当セクションを省く。`scope=Dm` では共有ノートを注入しない。
#[must_use]
pub fn build_guild_system_instruction(
    persona_prompt: Option<&str>,
    scope: GuildScope,
    speaker_display_name: &str,
    speaker_user_id: &str,
    guild_note: &str,
    personal_note: &str,
    date_time_str: &str,
) -> String {
    let persona_section = match persona_prompt {
        Some(p) if !p.trim().is_empty() => {
            format!("# あなたの役割・キャラクター設定（ペルソナ）\n{}", p.trim())
        }
        _ => DEFAULT_PERSONA.to_owned(),
    };
    let mode_section = match scope {
        GuildScope::Guild => MODE_GUILD,
        GuildScope::Dm => MODE_DM,
    };
    let rules = system_rules(date_time_str);

    // 共有ノート（guild のみ・ペルソナの後に注入）。
    let guild_note_section = if scope == GuildScope::Guild && !guild_note.trim().is_empty() {
        format!(
            "\n# 共有ノート（このサーバーの利用メンバー全員と共有している知識）\nサーバーのルール・用語・運用手順などの共有知識です。会話・判断の際に常に考慮してください。\n{}",
            guild_note.trim()
        )
    } else {
        String::new()
    };
    // 発話者の個人ノート（本人のプロンプトにのみ注入）。
    let personal_note_section = if personal_note.trim().is_empty() {
        String::new()
    } else {
        format!(
            "\n# 発話者の個人ノート（{speaker_display_name} さん専用の記憶）\n以下は現在の発話者本人に関する情報です。他のメンバーには開示しないでください。\n{}",
            personal_note.trim()
        )
    };
    let speaker_section = format!(
        "\n# 現在の発話者\n- 名前: {speaker_display_name}\n- メンション表記: <@{speaker_user_id}>"
    );

    [
        persona_section.as_str(),
        mode_section,
        RICH_REPLY,
        rules.as_str(),
        guild_note_section.as_str(),
        personal_note_section.as_str(),
        speaker_section.as_str(),
    ]
    .iter()
    .filter(|p| !p.is_empty())
    .copied()
    .collect::<Vec<_>>()
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{build_guild_system_instruction, GuildScope};
    use crate::system_prompt::DEFAULT_PERSONA;

    #[test]
    fn guild_mode_includes_member_rules_and_shared_note() {
        let sys = build_guild_system_instruction(
            None,
            GuildScope::Guild,
            "たろう",
            "123",
            "サーバーのルール",
            "",
            "2026年1月1日 (木) 00時00分00秒",
        );
        assert!(sys.contains(DEFAULT_PERSONA));
        assert!(sys.contains("サーバー常駐アシスタント"));
        assert!(sys.contains("利用メンバー管理のルール"));
        assert!(sys.contains("共有ノート"));
        assert!(sys.contains("サーバーのルール"));
        assert!(sys.contains("未実行の完了報告の禁止"));
        assert!(sys.contains("<@123>"));
    }

    #[test]
    fn dm_mode_omits_shared_note_and_uses_persona() {
        let sys = build_guild_system_instruction(
            Some("私は猫のキャラです"),
            GuildScope::Dm,
            "owner",
            "999",
            "無視される共有ノート",
            "本人は毎朝散歩する",
            "d",
        );
        assert!(sys.contains("私は猫のキャラです"));
        assert!(!sys.contains(DEFAULT_PERSONA));
        assert!(sys.contains("owner との動作確認DM"));
        // DM では共有ノートを注入しない。
        assert!(!sys.contains("無視される共有ノート"));
        // 個人ノートは DM でも注入する。
        assert!(sys.contains("本人は毎朝散歩する"));
    }
}
