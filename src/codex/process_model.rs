//! Pure classification of a process snapshot into Codex Desktop / its helpers / other
//! Codex clients.
//!
//! Codex Desktop ships as the MSIX package `OpenAI.Codex_2p2nqsd0c76g0` whose main
//! executable is `app\ChatGPT.exe` — the same image name as the separate ChatGPT Desktop
//! package (`OpenAI.ChatGPT-Desktop_…`). Image names are therefore never used to find
//! Desktop processes; package identity is.

use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessEntry {
    pub pid: u32,
    pub parent_pid: u32,
    pub image_path: Option<String>,
    /// Package family name from the process token, if the process has package identity.
    pub package_family: Option<String>,
    /// Creation time (FILETIME ticks); 0 when unknown. Guards against PID reuse when
    /// following parent links.
    pub created_at: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Classification {
    /// Processes running with the Codex Desktop package identity.
    pub desktop: Vec<ProcessEntry>,
    /// Descendants of Desktop processes (bundled app-server `codex.exe`, node runtimes, …).
    pub helpers: Vec<ProcessEntry>,
    /// Other `codex.exe` processes (CLI, IDE extensions, browser plugin host) that share
    /// CODEX_HOME but are not owned by Desktop. Never terminated by the switcher.
    pub external_clients: Vec<ProcessEntry>,
}

/// Derives a package family name (`Name_PublisherId`) from an image path located in an
/// MSIX install directory (`…\WindowsApps\Name_Version_Arch_Resource_PublisherId\…`).
pub fn family_from_install_path(path: &str) -> Option<String> {
    let lower = path.to_ascii_lowercase().replace('/', "\\");
    let idx = lower.find("\\windowsapps\\")?;
    let rest = &path[idx + "\\windowsapps\\".len()..];
    let dir = rest.split(['\\', '/']).next()?;
    let parts: Vec<&str> = dir.split('_').collect();
    if parts.len() != 5 || parts[0].is_empty() || parts[4].is_empty() {
        return None;
    }
    Some(format!("{}_{}", parts[0], parts[4]))
}

fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

pub fn is_desktop_process(entry: &ProcessEntry, family: &str) -> bool {
    match &entry.package_family {
        Some(f) => f.eq_ignore_ascii_case(family),
        None => entry
            .image_path
            .as_deref()
            .and_then(family_from_install_path)
            .is_some_and(|f| f.eq_ignore_ascii_case(family)),
    }
}

pub fn classify(entries: &[ProcessEntry], family: &str) -> Classification {
    let desktop: Vec<ProcessEntry> = entries.iter().filter(|e| is_desktop_process(e, family)).cloned().collect();
    let roots: Vec<(u32, u64)> = desktop.iter().map(|e| (e.pid, e.created_at)).collect();
    let owned_entries = descendants_of_known(entries, &roots);
    let owned: HashSet<u32> = owned_entries.iter().map(|e| e.pid).collect();
    let helpers = owned_entries.into_iter().filter(|e| !roots.iter().any(|(pid, _)| *pid == e.pid)).collect();

    let external_clients = entries
        .iter()
        .filter(|e| !owned.contains(&e.pid))
        .filter(|e| e.image_path.as_deref().is_some_and(|p| file_name(p).eq_ignore_ascii_case("codex.exe")))
        .cloned()
        .collect();
    Classification { desktop, helpers, external_clients }
}

/// The single ownership rule: known roots plus every descendant whose creation time is not
/// earlier than its parent's (an earlier child means the parent PID was reused).
/// Used by [`classify`] on a fresh snapshot and by the controller to keep tracking helpers
/// after Desktop itself has exited.
///
/// `known` holds `(pid, created_at)` recorded earlier. A known PID that is now held by a
/// process with a different creation time was recycled and is ignored; a known PID that
/// is gone still anchors its orphaned children (e.g. the app-server after Desktop exits).
pub fn descendants_of_known(entries: &[ProcessEntry], known: &[(u32, u64)]) -> Vec<ProcessEntry> {
    let recycled = |pid: u32, created: u64| {
        entries.iter().any(|e| e.pid == pid && created != 0 && e.created_at != 0 && e.created_at != created)
    };
    let mut owned: HashMap<u32, u64> = known.iter().copied().filter(|&(pid, c)| !recycled(pid, c)).collect();
    loop {
        let mut grew = false;
        for e in entries {
            if owned.contains_key(&e.pid) || e.pid == e.parent_pid {
                continue;
            }
            if let Some(&parent_created) = owned.get(&e.parent_pid) {
                if parent_created == 0 || e.created_at == 0 || e.created_at >= parent_created {
                    owned.insert(e.pid, e.created_at);
                    grew = true;
                }
            }
        }
        if !grew {
            break;
        }
    }
    entries
        .iter()
        .filter(|e| owned.get(&e.pid).is_some_and(|&c| c == 0 || e.created_at == 0 || c == e.created_at))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orphaned_helpers_stay_owned_after_desktop_exits() {
        // Desktop 100 (created 10) exited; its app-server 200 and grandchild 300 remain.
        let entries = vec![p(200, 100, "C:\\a.exe", None, 20), p(300, 200, "C:\\b.exe", None, 30), p(400, 1, "C:\\c.exe", None, 5)];
        let owned: Vec<u32> = descendants_of_known(&entries, &[(100, 10), (200, 20)]).iter().map(|e| e.pid).collect();
        assert_eq!(owned, vec![200, 300]);
    }

    #[test]
    fn recycled_pid_and_its_children_are_not_owned() {
        // PID 100 now belongs to an unrelated process created later, with its own child.
        let entries = vec![p(100, 1, "C:\\x.exe", None, 50), p(500, 100, "C:\\y.exe", None, 60)];
        assert!(descendants_of_known(&entries, &[(100, 10)]).is_empty());
    }

    const FAMILY: &str = "OpenAI.Codex_2p2nqsd0c76g0";
    const CODEX_EXE: &str = r"C:\Program Files\WindowsApps\OpenAI.Codex_26.908.4834.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe";
    const CHATGPT_EXE: &str = r"C:\Program Files\WindowsApps\OpenAI.ChatGPT-Desktop_1.2026.190.0_x64__2p2nqsd0c76g0\ChatGPT.exe";

    fn p(pid: u32, ppid: u32, path: &str, family: Option<&str>, created: u64) -> ProcessEntry {
        ProcessEntry { pid, parent_pid: ppid, image_path: Some(path.into()), package_family: family.map(Into::into), created_at: created }
    }

    fn snapshot() -> Vec<ProcessEntry> {
        vec![
            p(100, 4, CODEX_EXE, Some(FAMILY), 10),
            p(101, 100, CODEX_EXE, Some(FAMILY), 11),
            p(102, 100, r"C:\Users\u\AppData\Local\OpenAI\Codex\bin\abc\codex.exe", None, 12),
            p(103, 102, r"C:\Users\u\AppData\Local\OpenAI\Codex\runtimes\n\node.exe", None, 13),
            p(200, 4, CHATGPT_EXE, Some("OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0"), 5),
            p(201, 200, CHATGPT_EXE, Some("OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0"), 6),
            p(300, 50, r"c:\Users\u\.kiro\extensions\openai.chatgpt\bin\codex.exe", None, 20),
            p(400, 4, r"C:\Windows\explorer.exe", None, 1),
        ]
    }

    #[test]
    fn never_selects_chatgpt_desktop() {
        let c = classify(&snapshot(), FAMILY);
        let all: Vec<u32> = c.desktop.iter().chain(&c.helpers).map(|e| e.pid).collect();
        assert!(!all.contains(&200) && !all.contains(&201));
        assert_eq!(c.desktop.iter().map(|e| e.pid).collect::<Vec<_>>(), vec![100, 101]);
    }

    #[test]
    fn helpers_are_descendants_and_others_are_external() {
        let c = classify(&snapshot(), FAMILY);
        let mut helpers: Vec<u32> = c.helpers.iter().map(|e| e.pid).collect();
        helpers.sort();
        assert_eq!(helpers, vec![102, 103]);
        assert_eq!(c.external_clients.iter().map(|e| e.pid).collect::<Vec<_>>(), vec![300]);
    }

    #[test]
    fn pid_reuse_is_not_treated_as_descendant() {
        // pid 500 claims parent 100 but was created before Desktop started: reused parent pid.
        let mut s = snapshot();
        s.push(p(500, 100, r"C:\tools\codex.exe", None, 1));
        let c = classify(&s, FAMILY);
        assert!(!c.helpers.iter().any(|e| e.pid == 500));
        assert!(c.external_clients.iter().any(|e| e.pid == 500));
    }

    #[test]
    fn family_from_path_when_identity_unavailable() {
        assert_eq!(family_from_install_path(CODEX_EXE).as_deref(), Some(FAMILY));
        assert_eq!(family_from_install_path(CHATGPT_EXE).as_deref(), Some("OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0"));
        assert_eq!(family_from_install_path(r"C:\Temp\OpenAI.Codex_1_x64__2p2nqsd0c76g0\ChatGPT.exe"), None);
        let mut s = vec![p(1, 0, CODEX_EXE, None, 1)];
        s[0].package_family = None;
        assert_eq!(classify(&s, FAMILY).desktop.len(), 1);
    }

    #[test]
    fn identity_wins_over_path() {
        // A process copied out of WindowsApps with a foreign identity is not Desktop.
        let s = vec![p(1, 0, CODEX_EXE, Some("Other.App_xyz"), 1)];
        assert!(classify(&s, FAMILY).desktop.is_empty());
    }
}
