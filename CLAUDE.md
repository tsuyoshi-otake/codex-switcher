# codex-switcher — notes for agents

Verified project knowledge. Where to change what: the table at the top of `docs/architecture.md`.
History with root causes: `.claude/memory/journal.md`.

## Commands (Git Bash)
- `timeout 900 cargo test` — 97 tests, including the fault-injection proptest and the exhaustive fault sweep.
- `timeout 900 cargo clippy --all-targets` — must stay clean.
- `cargo build --release`, then `"$LOCALAPPDATA/Programs/Inno Setup 6/ISCC.exe" //Q installer/codex-switcher.iss` → `target/installer/`.
- Read-only smoke tests: `target/release/codex-switch.exe list`, `current`.

## Safety rules (from the spec, still in force)
- Never log, print, or notify token values. When inspecting auth files, print key names and types only.
- No hot swap, no Codex patching, no duplicated CODEX_HOME. Never `taskkill /IM ChatGPT.exe`: Codex Desktop and ChatGPT Desktop share that image name. `codex/process_model.rs::owned_processes` requires a Codex installation/runtime executable and verified creation times/ancestry. Inherited package identity and parent links alone do not authorize terminating user applications.
- `codex-switch use` closes the user's Codex. Ask before running it. Auto mode also blocks it for the agent, so the user runs it.

## Public repository hygiene
- This repo is public. Commit as `167153320+tsuyoshi-otake@users.noreply.github.com` (repo-local git config), never a work email.
- Test fixtures and docs use `@example.com` placeholders, never real account emails. The acceptance accounts in the spec were once copied verbatim and forced a history rewrite.
- Before any push, run `git grep -iE "system-exe|@[a-z0-9.-]+\.(jp|com)" -- ':!LICENSE'` and check that every hit is a placeholder.

## Pitfalls already paid for
- `JournalStore::clear` must delete the journal before the backup. The reverse order left a journal without a backup (RecoveryBlocked), which the proptest found.
- User-facing texts belong only in `app/messages.rs`. Tray and CLI must not format outcomes themselves.
- Autostart: the registry Run value is the source of truth. `autostart_default_applied` in config.json only records the one-time default-on. Never re-enable after an opt-out. The installer only repoints an existing value.
- Git Bash: use `MSYS_NO_PATHCONV=1 reg query ... /v X`, otherwise `/v` is mangled into a path.
- `~/.claude.json` has keys that differ only in case. Windows PowerShell 5 `ConvertFrom-Json` fails on it; use `pwsh` with `-AsHashtable`.
- windows-sys: `ACCESS_ALLOWED_ACE_TYPE` is not in `Win32::Security` (a local const 0 is used).

## Rejected: Claude account switching (2026-09-13)
- The Desktop Code tab follows the Claude Desktop login (host-auth env), not `~/.claude/.credentials.json`.
- Packaged Claude Desktop deletes `CLAUDE_USER_DATA_DIR` before using it unless a signed `CLAUDE_CDP_AUTH` is present. Do not bypass this, and do not swap Chromium profile files.
- The official route is Desktop's `multiAccount` feature (server flag).
- Terminal `claude` alone would be feasible via `CLAUDE_CONFIG_DIR` + `claude auth login`, but the user declined it.
