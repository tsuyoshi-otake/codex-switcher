//! Read model shown by the tray and CLI. Contains no secrets.

use crate::auth::AccountIdentity;
use crate::codex::CredentialStoreMode;
use crate::profile::{plan_label, ProfileEntry, ProfileId};

#[derive(Clone, Debug, PartialEq)]
pub enum AuthState {
    /// No auth.json in CODEX_HOME (signed out, or Codex never ran).
    Missing,
    SignedIn(AccountIdentity),
    /// auth.json exists but is not a ChatGPT login we can switch (API key, unreadable…).
    Unrecognized,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccountView {
    pub id: ProfileId,
    pub email: String,
    pub plan: String,
    pub account_id: Option<String>,
    pub active: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CorruptedView {
    pub id: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Overview {
    pub accounts: Vec<AccountView>,
    pub corrupted: Vec<CorruptedView>,
    pub auth: AuthState,
    pub active_profile: Option<ProfileId>,
    pub store_mode: CredentialStoreMode,
    pub recovery_pending: bool,
    pub warnings: Vec<String>,
}

impl Overview {
    /// "email / plan" of the account currently in auth.json.
    pub fn active_label(&self) -> Option<String> {
        match &self.auth {
            AuthState::SignedIn(id) => Some(format!(
                "{} / {}",
                id.email.as_deref().unwrap_or("(no email)"),
                plan_label(id.plan_type.as_deref())
            )),
            _ => None,
        }
    }

    pub fn can_switch(&self) -> bool {
        self.store_mode.is_file() && !self.recovery_pending
    }
}

pub fn build_overview(
    entries: Vec<ProfileEntry>,
    auth: AuthState,
    store_mode: CredentialStoreMode,
    recovery_pending: bool,
    warnings: Vec<String>,
) -> Overview {
    let mut accounts = Vec::new();
    let mut corrupted = Vec::new();
    let mut active_profile = None;
    for entry in entries {
        match entry {
            ProfileEntry::Valid(p) => {
                let m = p.metadata;
                let active = active_profile.is_none()
                    && matches!(&auth, AuthState::SignedIn(id) if id.same_account(&m.identity));
                if active {
                    active_profile = Some(m.id.clone());
                }
                accounts.push(AccountView {
                    plan: m.plan_label(),
                    email: m.identity.email.clone().unwrap_or_else(|| "(no email)".into()),
                    account_id: m.identity.account_id.clone(),
                    id: m.id,
                    active,
                });
            }
            ProfileEntry::Corrupted { id, reason } => corrupted.push(CorruptedView { id: id.to_string(), reason }),
        }
    }
    Overview { accounts, corrupted, auth, active_profile, store_mode, recovery_pending, warnings }
}
