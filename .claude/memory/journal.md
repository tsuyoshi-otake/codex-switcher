# Journal (append-only)

## 2026-09-13 — v1 implementation (#1)

- **Work**: investigation of Codex Desktop MSIX/processes/auth.json, threat model, ADRs, switch transaction + recovery design, full implementation (core layers + raw Win32 platform layer + tray + CLI), docs.
- **Symptom (found by tests)**: fault-injection proptest failed only on some random seeds: `RecoveryBlocked("credential backup is missing")` for `op_index=32, start_fails=true, codex_running=true`.
- **Root cause**: `JournalStore::clear` removed the backup before the journal. A failure between the two left a journal (phase ≥ ReplacingCredentials) pointing to a deleted backup, so recovery refused even though auth.json was already the source.
- **Fix**: clear journal first, then backup (an orphaned encrypted backup is harmless and removed by `recover`); recovery also accepts "auth.json has source identity" without needing the backup.
- **Verification**: 25 consecutive proptest runs green; added deterministic sweep `every_single_fault_position_preserves_the_invariant` (60 fault positions × 64 flag combos); `cargo test` 87 passed; clippy clean; release `codex-switch current` reads the live account read-only.
- **Learning**: random-seed property tests without failure persistence can hide counterexamples between runs — pair them with an exhaustive sweep when the input space is small. For two-file commit/cleanup, delete the *referencing* record before the *referenced* data.
- **Other fixes**: `compiler_fence()` does not exist (use `std::hint::black_box`); `ACCESS_ALLOWED_ACE_TYPE` lives in `Win32_System_SystemServices` in windows-sys 0.61 (defined locally); two wrong test expectations (UTC timestamp of 1_789_000_000 is 2026-09-10T00:26:40Z; login runner is called once per attempt).
- **Not yet verified**: live end-to-end switch between the 3 acceptance accounts (closes the user's Codex; needs user go-ahead).

## 2026-09-13 — IRV: localize user texts and the process-ownership rule (#1)
- Symptom: changing a notification text required reading tray_host.rs (386 lines) and cli.rs; recovery texts lived only in tray_host, CLI printed `{outcome:?}`. The PID-reuse descendant rule existed twice (classify and descendants_of_known).
- Fix: `describe_recovery` / `describe_saved_profile` in app/messages.rs used by tray and CLI (WorkerResult Added/Imported merged into Saved); classify now derives helpers via descendants_of_known; test modules merged; "Where to start for a change" table in docs/architecture.md.
- Behavior change (user-visible, intentional): CLI add/import/recover output is now the same Japanese text as the tray.
- Verification: cargo clippy --all-targets clean, cargo test 87 passed, no leftover processes.
- Learning: when two front-ends show the same outcome, put the text in app/messages.rs from the start.

## 2026-09-13 — Live acceptance: 3 profiles (#1)
- Registered via `import` (account A, Personal Pro) and two `add` runs (account B Business, account B Personal Pro). The first `add` picked Business at the workspace chooser; the account/workspace choice is the user's in the browser.
- The user ran `use 3`, `use 2`, `use 1` and reported OK. Afterwards: `current` = profile 1, `list` shows no pending-recovery warning.
- Switching commands were blocked for the agent by the auto-mode classifier (closing Codex is outward-facing), so the user ran them.
- Autostart is off by default (HKCU Run value absent); it is enabled from the tray settings.

## 2026-09-13 — Autostart on by default (#1)
- Request: the user wanted autostart on by default (previously off until enabled in settings).
- Design: the registry Run value stays the source of truth for on/off. config.json gets a one-time marker `autostart_default_applied`. The first tray run calls `SwitcherService::apply_default_autostart`, which registers the Run value and sets the marker. Later runs never re-enable, so a user opt-out sticks. Failure is logged as a warning and retried on the next launch, because the marker is written only after a successful registration.
- Existing installs without the key are treated as a first run and get registered once.
- Verification: new test `autostart_is_on_by_default_once_and_a_later_opt_out_sticks`; cargo test 88 passed, clippy clean, release build ok, no leftover processes.

## 2026-09-13 — Investigation: Claude support, withdrawn (#1)
- Request: switch Claude accounts too. The user chose Claude Code, specifically including the Claude Desktop Code tab.
- Terminal Claude Code: `~/.claude/.credentials.json` (claudeAiOauth + organizationUuid), identity in `~/.claude.json` `oauthAccount` (mixed with ~65 other keys). `claude auth login|status --json` exists and the binary references CLAUDE_CONFIG_DIR, so an isolated-login design like Codex's is feasible.
- Code tab: sessions are children of Claude Desktop (`%APPDATA%\Claude\claude-code\<ver>\claude.exe`) and inherit host-auth env (`CLAUDE_CODE_SDK_HAS_HOST_AUTH_REFRESH`, `ANTHROPIC_BASE_URL`), so they follow the Desktop login, not `.credentials.json`.
- Desktop login lives in the Chromium profile (Cookies, Local Storage, IndexedDB, safeStorage-encrypted token store), mixed with unrelated state → file swapping rejected as unsafe.
- `CLAUDE_USER_DATA_DIR` is honored in code, but packaged builds `delete process.env.CLAUDE_USER_DATA_DIR` (asar offset 18022387) before `setPath("userData")` (18023043) unless a signed `CLAUDE_CDP_AUTH` is present (test harness). Bypassing it would defeat a deliberate control → rejected.
- An official `multiAccount` feature exists behind a server flag / `authentication.disableMultiAccount` policy.
- Decision: the user withdrew the request; no code changes. Learning: check the packaged-build env scrubbing before designing around an app's env var.

## 2026-09-13 — Local installer + install, CLAUDE.md (#1)
- Added `installer/codex-switcher.iss` (Inno Setup 6, per-user, PrivilegesRequired=lowest) → `target/installer/codex-switcher-setup-0.1.0.exe` (2.3 MB).
- Installed silently. Verified: files in `%LOCALAPPDATA%\Programs\CodexAccountSwitcher`, Start menu entries, uninstall entry, Run value repointed to the installed exe, installed `codex-switch.exe list` shows the 3 profiles.
- Design: setup only repoints an existing Run value, because the app owns the default-on and a user opt-out must stick. Uninstall removes the Run value but keeps the encrypted profiles.
- The old tray started from target\release (pid 6684) was left running; the installed exe is used from the next sign-in.
- Added repo `CLAUDE.md` with distilled, verified rules and pitfalls, at the user's request.

## 2026-09-14 — Public release prep: history rewrite (#1)
- Symptom: the pre-publish scan found a work email in all 6 commit authors and the three real acceptance account emails in `src/tray/menu.rs` tests; the journal also named the accounts.
- Root cause: (1) when the first commit failed with "Author identity unknown", the agent set the repo-local user.email to the user's work address instead of the GitHub noreply address; (2) the spec's acceptance accounts were copied verbatim into test fixtures.
- Fix: fixtures → `@example.com`, journal names anonymized, LICENSE (MIT) added, repo-local user.email = GitHub noreply, history squashed into one orphan commit and force-pushed before switching the repo to public.
- Learning (promoted to CLAUDE.md): never use real account emails in fixtures; set the noreply commit email before the first commit; scan with git grep before any push to a repo that may become public.
