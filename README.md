# codex-switcher

Windows tray app (Rust + raw Win32, no Electron/Tauri) that switches the **OpenAI Codex Desktop** sign-in between saved accounts in one click.

It swaps only `%USERPROFILE%\.codex\auth.json`. It never duplicates `CODEX_HOME`, never patches Codex, never hot-swaps credentials under a running Codex, and never touches the separate ChatGPT Desktop app.

## Build

```bash
cargo build --release
```

Binary: `target\release\codex-switch.exe`. Requires a Rust toolchain for `x86_64-pc-windows-msvc`.

## Install

Build the per-user installer with [Inno Setup 6](https://jrsoftware.org/isinfo.php); no admin rights are needed.

```bash
"$LOCALAPPDATA/Programs/Inno Setup 6/ISCC.exe" //Q installer/codex-switcher.iss
```

Run `target\installer\codex-switcher-setup-0.1.0.exe`.

- Installs to `%LOCALAPPDATA%\Programs\CodexAccountSwitcher` and adds a Start menu entry.
- An existing autostart entry is repointed to the installed exe.
- Uninstall removes the program and the autostart entry. Encrypted profiles in `%LOCALAPPDATA%\CodexAccountSwitcher` are kept; delete that folder to remove them.

## Tray usage

1. Run `codex-switch.exe` (no arguments). The icon appears in the notification area.
2. Left or right click it to open the menu:
   - account list: `email    plan` (e.g. `Personal / Pro`, `Business`), ✓ = active
   - `アカウントを追加...`: official `codex login` in an isolated staging home, then saved
   - `現在のアカウントを登録`: saves the account Codex is signed in with right now
   - `Codexを開く` / `設定...` / `プロファイルを削除` / `終了`
3. Click an account. Codex closes, the credentials are swapped, Codex restarts, and a notification reports the result.

Tooltip: `Active: email / plan`.

First run: sign in to Codex normally, then choose `現在のアカウントを登録`. Add the other accounts with `アカウントを追加...`.

## CLI

```text
codex-switch list              saved profiles (number, id, email, plan, active)
codex-switch current           account in auth.json
codex-switch use <n|id|email>  switch (email only if unambiguous)
codex-switch add               official login, then save
codex-switch import            save the current auth.json as a profile
codex-switch remove <n|id>     delete a profile (not the active one)
codex-switch recover           finish/roll back an interrupted switch
```

The exe uses the GUI subsystem, so `cmd.exe` does not wait for it and output can interleave with the prompt. In PowerShell use `codex-switch list | Out-String`, or `start /wait` in cmd.

## Settings

The settings are in `%LOCALAPPDATA%\CodexAccountSwitcher\config.json`. The `設定...` window edits the common ones.

| Key | Default | Meaning |
|---|---|---|
| `restart_codex_after_switch` | true | Start Codex again if it was running |
| `notifications` | true | Success balloons (failures always shown) |
| `log_level` | info | error / warn / info / debug (tokens are never logged at any level) |
| `graceful_stop_timeout_secs` | 10 | Wait after WM_CLOSE |
| `force_stop` / `force_stop_timeout_secs` | true / 5 | Terminate verified Codex-owned processes only |
| `external_clients_policy` | warn | `block` refuses to switch while other `codex.exe` clients (IDE/CLI) run |
| `codex_cli_path` | auto | Override the `codex.exe` used for login |

Start with Windows: the settings checkbox writes `HKCU\...\Run\CodexAccountSwitcher`.

## Data locations

`%LOCALAPPDATA%\CodexAccountSwitcher\`

- `profiles\<uuid>.profile`: DPAPI-encrypted credentials + display metadata
- `switch-journal.json` / `switch-backup.bin` exist only during a switch (the backup is DPAPI-encrypted)
- `logs\codex-switch.log`, `config.json`

## Docs

- [docs/investigation.md](docs/investigation.md): how the real Codex Desktop behaves
- [docs/architecture.md](docs/architecture.md): layers, switch transaction, recovery, ADRs
- [docs/security.md](docs/security.md): threat model and mitigations

## Tests

```bash
cargo test
```

The suite covers:
- the transaction state machine, with fault injection on every file operation (random proptest plus an exhaustive sweep);
- crash recovery;
- the profile vault;
- real DPAPI, the real atomic replace, junction refusal, and ACL detection;
- process identification;
- the menu, CLI, and settings.
