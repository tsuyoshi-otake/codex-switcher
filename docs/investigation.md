# Investigation: Codex Desktop on Windows (2026-09)

Observed on Windows 11 Pro 26200 with Codex Desktop from the Microsoft Store. Where these observations differ from the original spec, the implementation follows the observed behaviour.

## Packaging and processes

| Item | Observed |
|---|---|
| Package family | `OpenAI.Codex_2p2nqsd0c76g0` (MSIX) |
| Main executable | `…\WindowsApps\OpenAI.Codex_<ver>_x64__2p2nqsd0c76g0\app\ChatGPT.exe` |
| AUMID | `OpenAI.Codex_2p2nqsd0c76g0!App` → start via `shell:AppsFolder\…` |
| Child processes | Electron children (same package identity), app-server `%LOCALAPPDATA%\OpenAI\Codex\bin\<hash>\codex.exe`, `node.exe` runtimes (no package identity, descendants) |
| ChatGPT Desktop | `OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0`, **also `ChatGPT.exe`** |

Consequences:
- The image name cannot identify Codex, so `taskkill /IM ChatGPT.exe` would kill ChatGPT Desktop too.
- Identification uses the package family from the process token (`GetPackageFamilyName`), falling back to the WindowsApps install path.
- Helpers are found by parent links, with creation-time checks against PID reuse.

## Other Codex clients

Standalone `codex.exe` processes also read and write `~/.codex`. Observed: a Chrome native-messaging host and IDE extensions (Kiro, VS Code).

They are not owned by Desktop and are **never killed**. By default the switcher warns about them; the `block` policy refuses to switch instead.

The Warn default is safe because Codex's token-refresh path reloads auth.json and only writes back if `account_id` still matches (`reload_if_account_id_matches`). A stale client therefore does not overwrite the new account.

## auth.json

Location: `%CODEX_HOME%\auth.json`, where CODEX_HOME defaults to `%USERPROFILE%\.codex`.

Top-level keys:
- `auth_mode`
- `OPENAI_API_KEY`
- `tokens`: `id_token`, `access_token`, `refresh_token`, `account_id`
- `last_refresh`

Display metadata comes from the unverified `id_token` JWT payload claims:
- `email`
- `https://api.openai.com/auth.chatgpt_plan_type`
- `chatgpt_account_id`
- `chatgpt_user_id`

The same email can map to several `account_id`s (Personal vs Business workspace). Profiles therefore use UUIDs, and account identity is `account_id` plus user id or email.

Codex rotates refresh tokens and rewrites auth.json **non-atomically**. The switcher therefore:
- always saves the live file into its profile before replacing it, so rotation is not lost;
- only replaces the file while Desktop is stopped.

`cli_auth_credentials_store` can be `file`, `keyring` or `auto`. v1 supports `file` only. With `keyring` there is no file to swap, and the switcher refuses to switch **before** stopping Codex.

## Login

`codex login` honours `CODEX_HOME`. Running it with `CODEX_HOME=<state>\login-staging` and `-c cli_auth_credentials_store="file"` produces a complete auth.json without touching the live one or stopping Codex. The staging directory is scrubbed after every attempt and at startup.
