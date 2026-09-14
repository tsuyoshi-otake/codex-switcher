//! User-facing (Japanese) texts for notifications and dialogs. Built only from error
//! variants and non-secret metadata; never from credential content.

use crate::error::Error;
use crate::profile::ProfileMetadata;
use crate::switch::{RecoveryOutcome, RollbackStatus, SwitchFailure, SwitchOutcome};

/// `None` when there is nothing worth telling the user (no interrupted switch).
pub fn describe_recovery(o: &RecoveryOutcome) -> Option<&'static str> {
    match o {
        RecoveryOutcome::Clean => None,
        RecoveryOutcome::Discarded => Some("中断された切り替えの記録を破棄しました（認証情報は変更されていません）。"),
        RecoveryOutcome::RolledForward { .. } => Some("中断された切り替えを完了しました。"),
        RecoveryOutcome::RolledBack => Some("中断された切り替えを取り消し、元のアカウントに戻しました。"),
    }
}

/// Result of `add` / `import`: `created` distinguishes a new profile from a refreshed one.
pub fn describe_saved_profile(meta: &ProfileMetadata, created: bool) -> String {
    let verb = if created { "保存しました" } else { "更新しました" };
    format!("{verb}: {} / {}", meta.title(), meta.plan_label())
}

pub fn describe_error(e: &Error) -> String {
    match e {
        Error::Io { context, .. } => format!("ファイル操作に失敗しました（{context}）"),
        Error::Protect(_) => "資格情報の暗号化または復号に失敗しました（別のWindowsユーザーで作成されたプロファイルの可能性があります）".into(),
        Error::InvalidCredentials(r) => format!("認証情報を認識できません: {r}"),
        Error::UnsupportedCredentialStore(m) => {
            format!("Codexの資格情報ストア「{m}」には未対応です（config.toml の cli_auth_credentials_store が file の場合のみ対応）")
        }
        Error::ProfileNotFound(id) => format!("プロファイルが見つかりません: {id}"),
        Error::ProfileCorrupted { id, .. } => format!("プロファイルが破損しています: {id}"),
        Error::CurrentProfileRemoval => "現在使用中のアカウントは削除できません".into(),
        Error::CodexStop(r) => format!("Codexを終了できませんでした: {r}"),
        Error::CodexStart(r) => format!("Codexを起動できませんでした: {r}"),
        Error::ExternalClientsRunning(n) => format!("Codex CLIやIDE拡張などのcodexプロセスが{n}個実行中のため中止しました"),
        Error::RecoveryPending => "中断された切り替えがあります。メニューの「中断された切り替えを復旧」を実行してください".into(),
        Error::RecoveryBlocked(r) => format!("自動復旧できません: {r}"),
        Error::SwitchInProgress => "別の切り替え処理が実行中です".into(),
        Error::UnsafePath(p) => format!("安全でないパスを検出したため中止しました: {p}"),
        Error::Login(r) => format!("ログインに失敗しました: {r}"),
        Error::Config(r) => format!("設定エラー: {r}"),
        Error::Platform(r) => format!("Windows APIエラー: {r}"),
    }
}

pub fn describe_failure(f: &SwitchFailure) -> String {
    let rollback = match &f.rollback {
        RollbackStatus::NotNeeded => "認証情報は変更していません。".to_string(),
        RollbackStatus::Restored => "元のアカウントに戻しました。".to_string(),
        RollbackStatus::Failed(r) => format!("元に戻せませんでした（{r}）。次回起動時に復旧します。"),
    };
    format!("切り替えに失敗しました（{}）: {}\n{}", f.phase, describe_error(&f.error), rollback)
}

pub fn describe_outcome(o: &SwitchOutcome) -> String {
    let mut text = if o.already_active {
        format!("既に使用中です: {} / {}", o.target.title(), o.target.plan_label())
    } else {
        format!("切り替えました: {} / {}", o.target.title(), o.target.plan_label())
    };
    if o.codex_restarted {
        text.push_str("\nCodexを再起動しました。");
    }
    if o.source_registered {
        text.push_str("\n直前のアカウントを新しいプロファイルとして保存しました。");
    }
    if o.external_clients > 0 {
        text.push_str(&format!("\n他のcodexプロセス（CLI/IDE拡張）が{}個動作中です。再起動を推奨します。", o.external_clients));
    }
    text
}
