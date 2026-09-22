//! Pure classification of a process snapshot into Codex Desktop / its helpers / other
//! Codex clients.
//!
//! Codex Desktop ships as the MSIX package `OpenAI.Codex_2p2nqsd0c76g0` whose main
//! executable is `app\ChatGPT.exe` — the same image name as the separate ChatGPT Desktop
//! package (`OpenAI.ChatGPT-Desktop_…`). Image names are therefore never used to find
//! Desktop processes. Executable location must also agree with package identity:
//! user applications launched by Desktop can inherit its identity and parent links.

use std::collections::{HashMap, HashSet, VecDeque};

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
    /// Processes running from the Codex Desktop installation, with no conflicting identity.
    pub desktop: Vec<ProcessEntry>,
    /// Desktop descendants running from its dedicated bin/runtimes cache.
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
    entry.created_at != 0
        && compatible_identity(entry, family)
        && entry.image_path.as_deref().and_then(family_from_install_path).is_some_and(|f| f.eq_ignore_ascii_case(family))
}

fn compatible_identity(entry: &ProcessEntry, family: &str) -> bool {
    entry.package_family.as_ref().is_none_or(|f| f.eq_ignore_ascii_case(family))
}

fn is_runtime_process(entry: &ProcessEntry, family: &str, runtime_root: Option<&str>) -> bool {
    if entry.created_at == 0 || !compatible_identity(entry, family) {
        return false;
    }
    let (Some(path), Some(root)) = (entry.image_path.as_deref(), runtime_root) else {
        return false;
    };
    let path = path.to_ascii_lowercase().replace('/', "\\");
    let root = root.to_ascii_lowercase().replace('/', "\\");
    if root.is_empty() || path.split('\\').any(|part| matches!(part, "." | "..")) {
        return false;
    }
    let Some(relative) = path.strip_prefix(root.trim_end_matches('\\')).and_then(|p| p.strip_prefix('\\')) else {
        return false;
    };
    relative.starts_with("bin\\") || relative.starts_with("runtimes\\")
}

pub fn classify(entries: &[ProcessEntry], family: &str, runtime_root: Option<&str>) -> Classification {
    let desktop: Vec<ProcessEntry> = entries.iter().filter(|e| is_desktop_process(e, family)).cloned().collect();
    let roots: HashSet<u32> = desktop.iter().map(|e| e.pid).collect();
    let owned_entries = owned_processes(entries, &[], family, runtime_root);
    let owned: HashSet<u32> = owned_entries.iter().map(|e| e.pid).collect();
    let helpers = owned_entries.into_iter().filter(|e| !roots.contains(&e.pid)).collect();

    let external_clients = entries
        .iter()
        .filter(|e| !owned.contains(&e.pid))
        .filter(|e| e.image_path.as_deref().is_some_and(|p| file_name(p).eq_ignore_ascii_case("codex.exe")))
        .cloned()
        .collect();
    Classification { desktop, helpers, external_clients }
}

/// The ownership rule shared by inspection and every shutdown poll. Parentage establishes
/// reachability, not permission to terminate: only installed Desktop executables and its
/// dedicated runtime cache are selected. Shell wrappers can connect a runtime to Desktop
/// without becoming shutdown targets themselves. Missing images/timestamps fail closed.
/// Known helpers remain trackable after Desktop exits, but their images are revalidated.
pub fn owned_processes(entries: &[ProcessEntry], known: &[(u32, u64)], family: &str, runtime_root: Option<&str>) -> Vec<ProcessEntry> {
    let mut anchors = known.to_vec();
    anchors.extend(entries.iter().filter(|e| is_desktop_process(e, family)).map(|e| (e.pid, e.created_at)));
    descendants_of_known(entries, &anchors)
        .into_iter()
        .filter(|e| is_desktop_process(e, family) || is_runtime_process(e, family, runtime_root))
        .collect()
}

/// Reachable processes, including unowned intermediates. Never use this set to terminate.
///
/// `known` holds `(pid, created_at)` recorded earlier. A known PID that is now held by a
/// process with a different creation time was recycled and is ignored; a known PID that
/// is gone still anchors its orphaned children (e.g. the app-server after Desktop exits).
fn descendants_of_known(entries: &[ProcessEntry], known: &[(u32, u64)]) -> Vec<ProcessEntry> {
    let by_pid: HashMap<u32, &ProcessEntry> = entries.iter().map(|e| (e.pid, e)).collect();
    let mut children: HashMap<u32, Vec<&ProcessEntry>> = HashMap::new();
    for e in entries {
        children.entry(e.parent_pid).or_default().push(e);
    }
    let mut reachable: HashMap<u32, u64> =
        known.iter().copied().filter(|&(pid, created)| created != 0 && by_pid.get(&pid).is_none_or(|e| e.created_at == created)).collect();
    let mut queue: VecDeque<(u32, u64)> = reachable.iter().map(|(&pid, &created)| (pid, created)).collect();
    while let Some((parent, parent_created)) = queue.pop_front() {
        for e in children.get(&parent).into_iter().flatten() {
            if e.created_at != 0 && e.created_at >= parent_created && !reachable.contains_key(&e.pid) {
                reachable.insert(e.pid, e.created_at);
                queue.push_back((e.pid, e.created_at));
            }
        }
    }
    entries.iter().filter(|e| reachable.get(&e.pid) == Some(&e.created_at)).cloned().collect()
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
    const RUNTIME_ROOT: &str = r"C:\Users\u\AppData\Local\OpenAI\Codex";
    const CODEX_EXE: &str = r"C:\Program Files\WindowsApps\OpenAI.Codex_26.908.4834.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe";
    const CHATGPT_EXE: &str = r"C:\Program Files\WindowsApps\OpenAI.ChatGPT-Desktop_1.2026.190.0_x64__2p2nqsd0c76g0\ChatGPT.exe";

    fn classify(entries: &[ProcessEntry], family: &str) -> Classification {
        super::classify(entries, family, Some(RUNTIME_ROOT))
    }

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

    #[test]
    fn rundog_launched_by_codex_is_not_a_helper() {
        let mut entries = snapshot();
        entries.push(p(500, 102, r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe", None, 30));
        entries.push(p(501, 500, r"C:\Users\u\AppData\Local\Programs\RunDog\RunDog.exe", None, 31));
        let c = classify(&entries, FAMILY);
        assert!(!c.helpers.iter().any(|e| e.pid == 501), "RunDog is a user app, not a Codex helper");
        assert!(!c.helpers.iter().any(|e| e.pid == 500), "user shells must not be terminated");
    }

    #[test]
    fn inherited_package_identity_does_not_make_rundog_desktop() {
        let entries = vec![p(501, 500, r"C:\Users\u\AppData\Local\Programs\RunDog\RunDog.exe", Some(FAMILY), 31)];
        let c = classify(&entries, FAMILY);
        assert!(c.desktop.is_empty(), "package identity alone does not prove ownership of the executable");
        assert!(c.helpers.is_empty());
    }

    #[test]
    fn runtime_behind_shell_is_selected_but_shell_and_user_apps_are_not() {
        let mut entries = snapshot();
        entries.push(p(500, 102, r"C:\Windows\System32\cmd.exe", Some(FAMILY), 30));
        entries.push(p(501, 500, &format!(r"{RUNTIME_ROOT}\runtimes\cua_node\abc\node.exe"), Some(FAMILY), 31));
        entries.push(p(502, 501, r"C:\tools\RunDog.exe", Some(FAMILY), 32));
        entries.push(p(503, 501, r"C:\project\node_modules\node.exe", None, 32));
        // A CLI launched from a task is still external even though it descends from Desktop.
        entries.push(p(504, 500, r"C:\tools\codex.exe", Some(FAMILY), 32));
        entries.reverse();
        let c = classify(&entries, FAMILY);
        let helpers: HashSet<u32> = c.helpers.iter().map(|e| e.pid).collect();
        assert_eq!(helpers, HashSet::from([102, 103, 501]));
        assert_eq!(c.external_clients.iter().map(|e| e.pid).collect::<HashSet<_>>(), HashSet::from([300, 504]));
    }

    #[test]
    fn shutdown_polls_preserve_user_apps_and_keep_orphaned_and_new_helpers() {
        let initial = snapshot();
        let c = classify(&initial, FAMILY);
        let known: Vec<_> = c.desktop.iter().chain(&c.helpers).map(|e| (e.pid, e.created_at)).collect();
        // Desktop exits, app-server remains and spawns a user shell, a runtime, and RunDog.
        let mut later: Vec<_> = initial.into_iter().filter(|e| ![100, 101].contains(&e.pid)).collect();
        later.push(p(500, 102, r"C:\Windows\System32\cmd.exe", Some(FAMILY), 30));
        later.push(p(501, 500, &format!(r"{RUNTIME_ROOT}\bin\abc\codex-code-mode-host.exe"), None, 31));
        later.push(p(502, 500, r"C:\tools\RunDog.exe", Some(FAMILY), 32));
        let alive = owned_processes(&later, &known, FAMILY, Some(RUNTIME_ROOT));
        assert_eq!(alive.iter().map(|e| e.pid).collect::<Vec<_>>(), vec![102, 103, 501]);
        let remembered: Vec<_> = alive.iter().map(|e| (e.pid, e.created_at)).collect();
        later.retain(|e| e.pid == 502);
        assert!(owned_processes(&later, &remembered, FAMILY, Some(RUNTIME_ROOT)).is_empty(), "a resident app cannot keep shutdown pending");
    }

    #[test]
    fn shutdown_polls_discover_new_desktop_roots_and_their_helpers_together() {
        let entries = snapshot();
        let owned = owned_processes(&entries, &[], FAMILY, Some(RUNTIME_ROOT));
        assert_eq!(owned.iter().map(|e| e.pid).collect::<Vec<_>>(), vec![100, 101, 102, 103]);
    }

    #[test]
    fn runtime_location_requires_a_full_directory_boundary_and_ancestry() {
        for path in [
            format!(r"{RUNTIME_ROOT}-other\bin\codex.exe"),
            format!(r"{RUNTIME_ROOT}\bin-other\codex.exe"),
            format!(r"{RUNTIME_ROOT}\RunDog.exe"),
            format!(r"{RUNTIME_ROOT}\bin\..\..\RunDog.exe"),
            r"C:\Users\other\AppData\Local\OpenAI\Codex\bin\codex.exe".into(),
        ] {
            let mut entries = snapshot();
            entries.push(p(500, 102, &path, Some(FAMILY), 30));
            assert!(!classify(&entries, FAMILY).helpers.iter().any(|e| e.pid == 500), "{path}");
        }
        let entries = vec![p(500, 1, &format!(r"{RUNTIME_ROOT}\bin\abc\codex.exe"), Some(FAMILY), 30)];
        let c = classify(&entries, FAMILY);
        assert!(c.desktop.is_empty() && c.helpers.is_empty());
        assert_eq!(c.external_clients.len(), 1);

        let mut entries = snapshot();
        entries.push(p(500, 102, &format!("{RUNTIME_ROOT}/RUNTIMES/abc/node.exe").to_uppercase(), None, 30));
        assert!(classify(&entries, FAMILY).helpers.iter().any(|e| e.pid == 500));
    }

    #[test]
    fn missing_image_or_timestamp_and_conflicting_identity_are_never_targets() {
        let mut entries = snapshot();
        entries.push(p(500, 102, CODEX_EXE, Some(FAMILY), 30));
        entries.last_mut().unwrap().image_path = None;
        entries.push(p(501, 102, CODEX_EXE, Some(FAMILY), 0));
        entries.push(p(502, 102, &format!(r"{RUNTIME_ROOT}\bin\abc\codex.exe"), None, 0));
        entries.push(p(503, 102, &format!(r"{RUNTIME_ROOT}\bin\abc\codex.exe"), Some("Other.App_xyz"), 30));
        let known: Vec<_> = entries.iter().map(|e| (e.pid, e.created_at)).collect();
        assert!(owned_processes(&entries, &known, FAMILY, Some(RUNTIME_ROOT)).iter().all(|e| e.pid < 500));
        assert!(super::classify(&snapshot(), FAMILY, None).helpers.is_empty());
    }

    #[test]
    fn recycled_helper_pid_cannot_anchor_a_new_runtime() {
        let old = p(102, 100, &format!(r"{RUNTIME_ROOT}\bin\abc\codex.exe"), None, 12);
        let entries = vec![
            p(102, 1, r"C:\tools\RunDog.exe", Some(FAMILY), 40),
            p(500, 102, &format!(r"{RUNTIME_ROOT}\runtimes\abc\node.exe"), None, 41),
        ];
        assert!(owned_processes(&entries, &[(old.pid, old.created_at)], FAMILY, Some(RUNTIME_ROOT)).is_empty());
        assert!(descendants_of_known(&entries, &[(102, 0)]).is_empty());
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(2048))]
        #[test]
        fn user_app_chains_never_become_shutdown_targets(
            depth in 1u32..80,
            inherited in proptest::bool::ANY,
            desktop_exited in proptest::bool::ANY,
            reverse in proptest::bool::ANY,
        ) {
            let mut entries = snapshot();
            let known: Vec<_> = owned_processes(&entries, &[], FAMILY, Some(RUNTIME_ROOT))
                .iter().map(|e| (e.pid, e.created_at)).collect();
            for i in 0..depth {
                entries.push(p(1000 + i, if i == 0 { 102 } else { 999 + i },
                    r"C:\Users\u\AppData\Local\Programs\RunDog\RunDog.exe", inherited.then_some(FAMILY), 100 + i as u64));
            }
            if desktop_exited { entries.retain(|e| ![100, 101].contains(&e.pid)); }
            if reverse { entries.reverse(); }
            let owned = owned_processes(&entries, &known, FAMILY, Some(RUNTIME_ROOT));
            proptest::prop_assert!(owned.iter().all(|e| e.pid < 1000));
            proptest::prop_assert!(owned.iter().any(|e| e.pid == 102));
            proptest::prop_assert!(owned.iter().any(|e| e.pid == 103));
        }
    }
}
