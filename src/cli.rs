//! Command-line front-end: `codex-switch [tray|list|current|use <profile>|add|import|remove <profile>|recover]`.

use std::io::Write;

use crate::app::messages::{describe_error, describe_failure, describe_outcome, describe_recovery, describe_saved_profile};
use crate::app::{AuthState, Overview, SwitcherService};
use crate::error::{Error, Result};
use crate::profile::ProfileId;

pub const USAGE: &str = "\
Usage: codex-switch [command]

  (no command) | tray   run the tray application
  list                  list profiles (* = active)
  current               show the account Codex Desktop is using
  use <profile>         switch to a profile (number from `list`, id, id prefix, or email)
  add                   sign in with the official Codex login and save the account
  import                save the currently signed-in account as a profile
  remove <profile>      delete a profile (the active one cannot be removed)
  recover               finish or roll back an interrupted switch
";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliCommand {
    Tray,
    List,
    Current,
    Use(String),
    Add,
    Import,
    Remove(String),
    Recover,
    Help,
}

pub fn parse(args: &[String]) -> std::result::Result<CliCommand, String> {
    let rest = args.get(1..).unwrap_or_default();
    let one_arg = |name: &str| match rest {
        [value] => Ok(value.clone()),
        _ => Err(format!("`{name}` takes exactly one <profile> argument")),
    };
    let no_args = |cmd: CliCommand| if rest.is_empty() { Ok(cmd) } else { Err("unexpected extra arguments".to_string()) };
    match args.first().map(String::as_str) {
        None | Some("tray") => no_args(CliCommand::Tray),
        Some("list" | "ls") => no_args(CliCommand::List),
        Some("current") => no_args(CliCommand::Current),
        Some("use" | "switch") => one_arg("use").map(CliCommand::Use),
        Some("add") => no_args(CliCommand::Add),
        Some("import") => no_args(CliCommand::Import),
        Some("remove" | "rm") => one_arg("remove").map(CliCommand::Remove),
        Some("recover") => no_args(CliCommand::Recover),
        Some("help" | "-h" | "--help" | "/?") => Ok(CliCommand::Help),
        Some(other) => Err(format!("unknown command `{other}`")),
    }
}

/// Resolves a user query to exactly one profile.
pub fn resolve_profile(ov: &Overview, query: &str) -> Result<ProfileId> {
    let q = query.trim();
    if let Ok(n) = q.parse::<usize>() {
        if let Some(a) = n.checked_sub(1).and_then(|i| ov.accounts.get(i)) {
            return Ok(a.id.clone());
        }
    }
    let lower = q.to_ascii_lowercase();
    if let Some(a) = ov.accounts.iter().find(|a| a.id.as_str() == lower) {
        return Ok(a.id.clone());
    }
    let pick = |matches: Vec<&ProfileId>| -> Option<Result<ProfileId>> {
        match matches.as_slice() {
            [] => None,
            [one] => Some(Ok((*one).clone())),
            _ => Some(Err(Error::Config(format!("`{q}` matches {} profiles; use the number or id", matches.len())))),
        }
    };
    if lower.len() >= 4 {
        if let Some(r) = pick(ov.accounts.iter().filter(|a| a.id.as_str().starts_with(&lower)).map(|a| &a.id).collect()) {
            return r;
        }
    }
    if let Some(r) = pick(ov.accounts.iter().filter(|a| a.email.eq_ignore_ascii_case(q)).map(|a| &a.id).collect()) {
        return r;
    }
    Err(Error::ProfileNotFound(q.to_string()))
}

fn print_list(ov: &Overview, out: &mut dyn Write) -> std::io::Result<()> {
    if ov.accounts.is_empty() {
        writeln!(out, "No profiles. Run `codex-switch add` or `codex-switch import`.")?;
    }
    for (i, a) in ov.accounts.iter().enumerate() {
        writeln!(out, "{} {}. {:<40} {:<18} {}", if a.active { '*' } else { ' ' }, i + 1, a.email, a.plan, a.id)?;
    }
    for c in &ov.corrupted {
        writeln!(out, "  !  corrupted profile {} ({})", c.id, c.reason)?;
    }
    for w in &ov.warnings {
        writeln!(out, "warning: {w}")?;
    }
    if ov.recovery_pending {
        writeln!(out, "warning: an interrupted switch is pending; run `codex-switch recover`")?;
    }
    Ok(())
}

/// Runs a non-tray command. Returns the process exit code.
pub fn run(cmd: &CliCommand, svc: &SwitcherService, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
    let result: Result<()> = (|| {
        let io = |e: std::io::Error| Error::io("writing to console", e);
        match cmd {
            CliCommand::Tray => unreachable!("tray is handled by main"),
            CliCommand::Help => write!(out, "{USAGE}").map_err(io),
            CliCommand::List => print_list(&svc.overview()?, out).map_err(io),
            CliCommand::Current => {
                let ov = svc.overview()?;
                match (&ov.auth, ov.active_label()) {
                    (AuthState::SignedIn(_), Some(label)) => {
                        let id = ov.active_profile.as_ref().map_or("not registered".to_string(), |p| p.to_string());
                        writeln!(out, "{label} ({id})").map_err(io)
                    }
                    (AuthState::Unrecognized, _) => writeln!(out, "auth.json is present but not a ChatGPT login").map_err(io),
                    _ => writeln!(out, "Codex is not signed in").map_err(io),
                }
            }
            CliCommand::Use(q) => {
                let id = resolve_profile(&svc.overview()?, q)?;
                let outcome = svc.switch_to(&id, &mut |phase| {
                    let _ = writeln!(err, "... {phase}");
                });
                match outcome {
                    Ok(o) => writeln!(out, "{}", describe_outcome(&o)).map_err(io),
                    Err(f) => {
                        let _ = writeln!(err, "{}", describe_failure(&f));
                        Err(f.error)
                    }
                }
            }
            CliCommand::Add => {
                writeln!(err, "Opening the official Codex sign-in. Complete it in your browser...").map_err(io)?;
                let (m, created) = svc.add_account()?;
                writeln!(out, "{} ({})", describe_saved_profile(&m, created), m.id).map_err(io)
            }
            CliCommand::Import => {
                let (m, created) = svc.import_current()?;
                writeln!(out, "{} ({})", describe_saved_profile(&m, created), m.id).map_err(io)
            }
            CliCommand::Remove(q) => {
                let id = resolve_profile(&svc.overview()?, q)?;
                svc.remove_profile(&id)?;
                writeln!(out, "Removed {id}").map_err(io)
            }
            CliCommand::Recover => {
                let outcome = svc.recover()?;
                writeln!(out, "{}", describe_recovery(&outcome).unwrap_or("中断された切り替えはありません。")).map_err(io)
            }
        }
    })();
    match result {
        Ok(()) => 0,
        Err(e) => {
            let _ = writeln!(err, "error: {e}\n{}", describe_error(&e));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::overview::build_overview;
    use crate::app::AccountView;
    use crate::codex::CredentialStoreMode;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn parses_commands() {
        assert_eq!(parse(&[]), Ok(CliCommand::Tray));
        assert_eq!(parse(&args("list")), Ok(CliCommand::List));
        assert_eq!(parse(&args("use 2")), Ok(CliCommand::Use("2".into())));
        assert!(parse(&args("use")).is_err());
        assert!(parse(&args("list extra")).is_err());
        assert!(parse(&args("frobnicate")).is_err());
    }

    fn ov() -> Overview {
        let mut o = build_overview(vec![], crate::app::AuthState::Missing, CredentialStoreMode::File, false, vec![]);
        for (n, email, plan) in [(1u8, "a@example.com", "Personal / Pro"), (2, "t@example.com", "Personal / Pro"), (3, "t@example.com", "Business")] {
            o.accounts.push(AccountView { id: ProfileId::from_random_bytes([n; 16]), email: email.into(), plan: plan.into(), account_id: None, active: false });
        }
        o
    }

    #[test]
    fn resolves_by_number_id_prefix_and_unique_email() {
        let o = ov();
        assert_eq!(resolve_profile(&o, "3").unwrap(), o.accounts[2].id);
        assert_eq!(resolve_profile(&o, &o.accounts[1].id.to_string().to_uppercase()).unwrap(), o.accounts[1].id);
        assert_eq!(resolve_profile(&o, &o.accounts[0].id.as_str()[..6]).unwrap(), o.accounts[0].id);
        assert_eq!(resolve_profile(&o, "A@EXAMPLE.COM").unwrap(), o.accounts[0].id);
        assert!(matches!(resolve_profile(&o, "t@example.com"), Err(Error::Config(_))), "same email twice is ambiguous");
        assert!(matches!(resolve_profile(&o, "9"), Err(Error::ProfileNotFound(_))));
    }
}
