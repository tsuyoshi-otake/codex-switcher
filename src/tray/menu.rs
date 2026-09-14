use crate::app::{AccountView, AuthState, Overview};
use crate::profile::ProfileId;

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    SwitchTo(ProfileId),
    AddAccount,
    ImportCurrent,
    OpenCodex,
    Settings,
    RemoveProfile(ProfileId),
    RemoveCorrupted(String),
    Recover,
    Quit,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    pub label: String,
    pub command: Option<Command>,
    pub checked: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MenuEntry {
    Item(MenuItem),
    Separator,
    Submenu { label: String, entries: Vec<MenuEntry> },
}

fn item(label: impl Into<String>, command: Option<Command>, enabled: bool) -> MenuEntry {
    MenuEntry::Item(MenuItem { label: escape(&label.into()), command, checked: false, enabled })
}

fn info(label: impl Into<String>) -> MenuEntry {
    item(label, None, false)
}

/// `&` is the mnemonic marker in Win32 menus.
fn escape(s: &str) -> String {
    s.replace('&', "&&")
}

/// "email<TAB>plan"; the tab right-aligns the plan column. Accounts sharing email and plan
/// (two Business workspaces) get the workspace id tail appended.
fn account_label(a: &AccountView, all: &[AccountView]) -> String {
    let collides = all.iter().filter(|o| o.email.eq_ignore_ascii_case(&a.email) && o.plan == a.plan).count() > 1;
    let tail = match (&a.account_id, collides) {
        (Some(acct), true) => {
            let chars: Vec<char> = acct.chars().collect();
            format!(" (…{})", chars[chars.len().saturating_sub(6)..].iter().collect::<String>())
        }
        _ => String::new(),
    };
    format!("{}\t{}{}", a.email, a.plan, tail)
}

/// `overview == None` means the state could not be read.
pub fn build_menu(overview: Option<&Overview>, busy: bool) -> Vec<MenuEntry> {
    let mut m = Vec::new();
    if busy {
        m.push(info("切り替え中..."));
        m.push(MenuEntry::Separator);
    }

    match overview {
        None => m.push(info("状態を読み込めません（ログを確認してください）")),
        Some(ov) => {
            if ov.recovery_pending {
                m.push(item("中断された切り替えを復旧", Some(Command::Recover), !busy));
                m.push(MenuEntry::Separator);
            }
            if !ov.store_mode.is_file() {
                m.push(info(format!("未対応の資格情報ストア: {}", ov.store_mode.name())));
            }
            if ov.accounts.is_empty() {
                m.push(info("（アカウント未登録）"));
            }
            let can_switch = ov.can_switch() && !busy;
            for a in &ov.accounts {
                m.push(MenuEntry::Item(MenuItem {
                    label: escape(&account_label(a, &ov.accounts)),
                    command: Some(Command::SwitchTo(a.id.clone())),
                    checked: a.active,
                    enabled: can_switch,
                }));
            }
            for c in &ov.corrupted {
                m.push(info(format!("破損したプロファイル: {}…", &c.id[..8.min(c.id.len())])));
            }
            match &ov.auth {
                AuthState::SignedIn(_) if ov.active_profile.is_none() => {
                    m.push(MenuEntry::Separator);
                    m.push(item("現在のアカウントを登録", Some(Command::ImportCurrent), !busy));
                }
                AuthState::Unrecognized => m.push(info("Codexの認証情報を認識できません")),
                _ => {}
            }
        }
    }

    m.push(MenuEntry::Separator);
    m.push(item("アカウントを追加...", Some(Command::AddAccount), !busy));
    m.push(item("Codexを開く", Some(Command::OpenCodex), true));

    if let Some(ov) = overview {
        let mut removable: Vec<MenuEntry> = ov
            .accounts
            .iter()
            .filter(|a| !a.active)
            .map(|a| item(account_label(a, &ov.accounts), Some(Command::RemoveProfile(a.id.clone())), !busy))
            .collect();
        removable.extend(ov.corrupted.iter().map(|c| item(format!("破損: {}", c.id), Some(Command::RemoveCorrupted(c.id.clone())), !busy)));
        if !removable.is_empty() {
            m.push(MenuEntry::Submenu { label: "プロファイルを削除".into(), entries: removable });
        }
    }

    m.push(item("設定...", Some(Command::Settings), true));
    m.push(MenuEntry::Separator);
    m.push(item("終了", Some(Command::Quit), true));
    m
}

/// Notification-area tooltip, limited to 127 UTF-16 units (NOTIFYICONDATAW::szTip).
pub fn tooltip(overview: Option<&Overview>, busy: bool) -> String {
    let text = match overview {
        _ if busy => "Codex Switcher: 切り替え中...".to_string(),
        None => "Codex Switcher: 状態を読み込めません".to_string(),
        Some(ov) => match (&ov.auth, ov.active_label()) {
            (AuthState::SignedIn(_), Some(label)) if ov.active_profile.is_some() => format!("Active: {label}"),
            (AuthState::SignedIn(_), Some(label)) => format!("Active: {label}（未登録）"),
            (AuthState::Unrecognized, _) => "Codex: 認識できない認証情報".to_string(),
            _ => "Codex: 未ログイン".to_string(),
        },
    };
    truncate_utf16(&text, 127)
}

fn truncate_utf16(s: &str, max_units: usize) -> String {
    if s.encode_utf16().count() <= max_units {
        return s.to_string();
    }
    let mut out = String::new();
    let mut units = 0;
    for c in s.chars() {
        if units + c.len_utf16() + 1 > max_units {
            break;
        }
        units += c.len_utf16();
        out.push(c);
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::overview::build_overview;
    use crate::auth::AccountIdentity;
    use crate::codex::CredentialStoreMode;
    use crate::profile::{ProfileEntry, ProfileMetadata, StoredProfile};

    fn identity(email: &str, acct: &str, plan: &str) -> AccountIdentity {
        AccountIdentity {
            email: Some(email.into()),
            plan_type: Some(plan.into()),
            account_id: Some(acct.into()),
            user_id: Some(format!("user-{email}")),
        }
    }

    fn entry(n: u8, id: AccountIdentity) -> ProfileEntry {
        ProfileEntry::Valid(StoredProfile {
            metadata: ProfileMetadata { id: ProfileId::from_random_bytes([n; 16]), identity: id, created_at: n as u64, updated_at: 0 },
            encrypted_credentials: vec![1],
        })
    }

    fn acceptance_overview(auth: AuthState, store: CredentialStoreMode) -> Overview {
        build_overview(
            vec![
                entry(1, identity("alice@example.com", "acct-n", "pro")),
                entry(2, identity("bob@example.com", "acct-p", "pro")),
                entry(3, identity("bob@example.com", "acct-b", "business")),
            ],
            auth,
            store,
            false,
            vec![],
        )
    }

    fn items(menu: &[MenuEntry]) -> Vec<&MenuItem> {
        menu.iter().filter_map(|e| if let MenuEntry::Item(i) = e { Some(i) } else { None }).collect()
    }

    #[test]
    fn acceptance_three_profiles_one_checked() {
        let ov = acceptance_overview(AuthState::SignedIn(identity("bob@example.com", "acct-b", "business")), CredentialStoreMode::File);
        let menu = build_menu(Some(&ov), false);
        let accounts: Vec<_> = items(&menu).into_iter().filter(|i| matches!(i.command, Some(Command::SwitchTo(_)))).collect();
        assert_eq!(
            accounts.iter().map(|i| (i.label.as_str(), i.checked, i.enabled)).collect::<Vec<_>>(),
            vec![
                ("alice@example.com\tPersonal / Pro", false, true),
                ("bob@example.com\tPersonal / Pro", false, true),
                ("bob@example.com\tBusiness", true, true),
            ]
        );
        assert_eq!(tooltip(Some(&ov), false), "Active: bob@example.com / Business");
        let labels: Vec<_> = items(&menu).iter().map(|i| i.label.clone()).collect();
        for required in ["アカウントを追加...", "Codexを開く", "設定...", "終了"] {
            assert!(labels.iter().any(|l| l == required), "missing {required}");
        }
    }

    #[test]
    fn busy_or_unsupported_store_disables_switching() {
        let ov = acceptance_overview(AuthState::Missing, CredentialStoreMode::Keyring);
        let menu = build_menu(Some(&ov), false);
        assert!(items(&menu).iter().filter(|i| matches!(i.command, Some(Command::SwitchTo(_)))).all(|i| !i.enabled));
        let ov = acceptance_overview(AuthState::Missing, CredentialStoreMode::File);
        let menu = build_menu(Some(&ov), true);
        assert!(items(&menu).iter().filter(|i| matches!(i.command, Some(Command::SwitchTo(_)))).all(|i| !i.enabled));
        assert_eq!(tooltip(Some(&ov), false), "Codex: 未ログイン");
    }

    #[test]
    fn active_profile_is_not_offered_for_removal() {
        let ov = acceptance_overview(AuthState::SignedIn(identity("alice@example.com", "acct-n", "pro")), CredentialStoreMode::File);
        let menu = build_menu(Some(&ov), false);
        let sub = menu.iter().find_map(|e| if let MenuEntry::Submenu { entries, .. } = e { Some(entries) } else { None }).unwrap();
        assert_eq!(sub.len(), 2);
        assert!(items(sub).iter().all(|i| !i.label.starts_with("alice")));
    }

    #[test]
    fn unregistered_active_account_offers_import() {
        let ov = acceptance_overview(AuthState::SignedIn(identity("new@example.com", "acct-x", "plus")), CredentialStoreMode::File);
        let menu = build_menu(Some(&ov), false);
        assert!(items(&menu).iter().any(|i| i.command == Some(Command::ImportCurrent)));
        assert_eq!(tooltip(Some(&ov), false), "Active: new@example.com / Personal / Plus（未登録）");
    }

    #[test]
    fn duplicate_email_and_plan_are_disambiguated_and_ampersand_escaped() {
        let ov = build_overview(
            vec![entry(1, identity("a&b@example.com", "workspace-111111", "business")), entry(2, identity("a&b@example.com", "workspace-222222", "business"))],
            AuthState::Missing,
            CredentialStoreMode::File,
            false,
            vec![],
        );
        let menu = build_menu(Some(&ov), false);
        let labels: Vec<_> = items(&menu).iter().filter(|i| matches!(i.command, Some(Command::SwitchTo(_)))).map(|i| i.label.clone()).collect();
        assert_eq!(labels, vec!["a&&b@example.com\tBusiness (…111111)", "a&&b@example.com\tBusiness (…222222)"]);
    }

    #[test]
    fn tooltip_is_truncated_to_win32_limit() {
        let long = "x".repeat(300);
        let ov = acceptance_overview(AuthState::SignedIn(identity(&long, "acct-z", "pro")), CredentialStoreMode::File);
        assert!(tooltip(Some(&ov), false).encode_utf16().count() <= 127);
    }
}
