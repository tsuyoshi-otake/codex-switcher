# Architecture

## Where to start for a change

Read the entry file and its tests first; the other layers should only be needed through their traits.

| Change | Entry point | Tests | Usually not needed |
|---|---|---|---|
| Switch order, rollback, invariant | `src/switch/transaction.rs` | `src/switch/tests.rs` | Win32, tray |
| Crash recovery rules | `src/switch/recovery.rs`, `journal.rs` | `src/switch/tests.rs` | transaction internals |
| Which processes are Codex | `src/codex/process_model.rs` (single ownership rule) | same file | `codex_controller.rs` |
| How Codex is stopped / started | `src/platform/windows/codex_controller.rs` | `CodexController` contract in `src/codex/controller.rs` | switch |
| Menu items, labels, tooltip | `src/tray/menu.rs` | same file | `tray_host.rs` (renders only) |
| Any user-facing text (tray and CLI) | `src/app/messages.rs` | — | front-ends |
| Account metadata (email/plan) | `src/auth/identity.rs` | same file | profile storage |
| Profile storage / encryption | `src/profile/vault.rs`, `platform/windows/dpapi.rs` | `profile/*`, `dpapi.rs` | switch |
| Settings keys | `src/config/mod.rs` | same file | settings window |
| Use-case wiring, locking | `src/app/service.rs` | `src/app/tests.rs` | platform |

## Layers (dependencies point downward only)

```
main.rs ─► platform::windows::bootstrap   (composition root)
             │
   tray_host / settings_window (Win32 UI)     cli.rs
             │  renders tray::menu (pure)       │
             └───────────────┬─────────────────┘
                     app::SwitcherService          (use cases, locking, messages)
                ┌────────────┼──────────────┐
            switch::*     profile::*      codex::*       (transaction/recovery, vault, lifecycle model)
                └────────────┼──────────────┘
          ports: FileStore, SecretProtector, CodexController, LoginRunner,
                 StartupRegistration, PermissionProbe, Ambient
                             │
            platform::windows::{fs, dpapi, process, codex_controller, login_cli, startup, acl, ambient}
```

- `tray` builds menu entries and tooltips from an `Overview` value. It never touches the filesystem, DPAPI, processes, or auth.json. `tray_host` only renders them and calls the service on worker threads.
- All `unsafe` is inside `platform::windows`.
- Every port has an in-memory fake in `src/testing` (`MemFs` with op-indexed fault and crash injection, `FakeProtector`, `FakeCodex`, `FakeLogin`).

## Process ownership (`codex/process_model.rs`)

`classify` and every shutdown poll share `owned_processes`. A target must have a
known creation time and an executable in the Codex MSIX installation, or in
`%LOCALAPPDATA%\OpenAI\Codex\bin` / `runtimes` with verified Desktop ancestry.
A conflicting package identity excludes it. Inherited identity or parentage
alone never makes a user application (such as RunDog) a shutdown target.

Ancestry traversal is O(N) and can cross a shell wrapper to find a bundled runtime,
without selecting the shell. Only eligible targets are remembered for later polls;
their executable and PID creation time are checked again. Known helpers can outlive
Desktop, but unrelated resident apps cannot keep shutdown pending. Other `codex.exe`
processes remain external clients, including CLIs started from a Desktop task.
Missing image/timestamp metadata excludes a process from termination. Future runtime
locations require updating this policy and its regression tests; do not restore an
unrestricted descendant rule.

## Switch transaction (`switch::transaction`)

Pre-flight checks, before anything is stopped:
- no pending journal;
- the target profile decrypts;
- the credential store is `file`;
- the external-client policy allows the switch.

Steps:

| # | Phase | Durable effect |
|---|---|---|
| 1 | StoppingCodex / WaitingForExit | WM_CLOSE → wait → optional terminate of verified Codex-owned PIDs → re-inspect |
| 2 | SavingCurrentCredentials | journal `Prepared` → DPAPI backup of live auth.json → journal `CredentialsBackedUp` → upsert into its profile (captures refresh-token rotation; auto-registers unknown accounts) |
| 3 | DecryptingTargetProfile | none |
| 4 | ReplacingCredentials | journal `ReplacingCredentials` → atomic replace → read back + constant-time compare → journal `CredentialsReplaced` |
| 5 | StartingCodex / Verifying | start via AUMID, wait for the Desktop process |
| 6 | Completed | clear journal, then backup |

Failure handling:
- A failure before step 4 leaves auth.json untouched: the journal is cleared and Codex is restarted.
- A failure in or after step 4 rolls back:
  1. stop Codex;
  2. if auth.json already holds the target, save it into the target profile;
  3. restore the backup, or delete auth.json if none existed;
  4. clear the journal;
  5. restart Codex (best effort).

**Invariant:** after any failure or crash plus `recover`, auth.json is byte-identical to the complete source or the complete target, the journal is gone, and the source's newest credentials exist in auth.json or its profile.

Tests check this property:
- a proptest (768 random cases per run);
- an exhaustive sweep of 60 fault positions × 64 flag combinations, each fault being an error or a crash.

## Recovery (`switch::recovery`, run at tray start and via `recover`)

1. No journal: remove any orphaned backup → `Clean`.
2. Phase < ReplacingCredentials: auth.json was not touched → discard.
3. auth.json has the target identity → roll forward.
4. auth.json has the source identity, equals the backup, or is absent when it never existed → rolled back.
5. Otherwise, e.g. an external modification: restore the backup only while Codex is not running; if it is running → `RecoveryBlocked`.

An unreadable or unknown-version journal → `RecoveryBlocked`; the switcher never guesses.

`JournalStore::clear` removes the journal **before** the backup. The opposite order could leave a journal whose backup is gone, which blocked recovery. The fault-injection proptest found this.

## Concurrency

- `switch.lock` (exclusive open, share mode 0) serializes switch, add, import, remove, and recover across tray and CLI processes.
- The `Local\CodexAccountSwitcher-Tray` mutex allows one tray per session.
- Tray UI state lives in a thread-local. Workers post a boxed result back with `WM_APP+2`. No borrow is held across modal loops (menu, MessageBox).

## ADRs

1. **Isolated login home instead of backup → login → restore.** `codex login` writes to a staging CODEX_HOME. The live auth.json and a running Codex are untouched, and there is nothing to roll back if the login is abandoned.
2. **No `state.json` for the current account.** "Active" is derived from auth.json identity every time, so it cannot drift when the user signs in inside Codex.
3. **Raw windows-sys, no GUI framework.** Small binary, no WebView, and the full API surface is auditable.
4. **Startup via the HKCU Run key.**
   - The Startup-folder shortcut needs COM `IShellLink`.
   - Task Scheduler is heavier and needs XML/COM.
   - MSIX StartupTask requires packaging.
   - The Run key is per-user, needs no elevation, and is visible in Task Manager › Startup apps.
   - On by default: the first tray run registers it once and records `autostart_default_applied` in config.json. Later runs never re-enable it, so turning it off sticks.
5. **External codex clients: warn by default, never kill.** See the refresh-guard note in investigation.md.
6. **File credential store only (v1).** Keyring mode is detected and refused before Codex is stopped.
7. **No idle detection.** A switch closes Codex just like the user closing it. An in-progress task is the user's call, because the switch is an explicit click.
8. **Failure notifications are always shown.** The `notifications` setting only suppresses success balloons: a silent failure could leave the user on the wrong account without knowing.
9. **CLI in the GUI-subsystem exe.** `AttachConsole(ATTACH_PARENT_PROCESS)` is used, so the tray never flashes a console. Limitation: cmd does not wait for the process.
