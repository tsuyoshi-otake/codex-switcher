# Security

## Assets

- ChatGPT OAuth tokens: refresh, access, and ID tokens, plus an optional API key, in auth.json and profiles.
- Integrity of the active sign-in: the user must know which account Codex uses.
- Unrelated processes and data (ChatGPT Desktop, IDE sessions).

## Trust boundary

The attacker is another local user, malware running as a *different* user, or a later reader of disk, backups, or logs.

Out of scope: malware already running as the same user. It can read `~/.codex/auth.json` directly and call DPAPI as that user.

## Threats and mitigations

| Threat | Mitigation |
|---|---|
| Profile files copied off the machine or read by another user | DPAPI `CryptProtectData`, current-user scope, `CRYPTPROTECT_UI_FORBIDDEN`, app-specific entropy; tampering fails decryption and the profile is shown as corrupted |
| Plaintext left in temp files | Atomic writes use a same-directory temp opened `create_new` with share mode 0; on failure it is zero-overwritten and deleted. Profile and backup contents are ciphertext. The login staging dir is scrubbed after each attempt and at startup. Deleted secrets are zero-overwritten first (best effort; SSD/NTFS may keep old clusters, which is why data at rest is encrypted) |
| Secrets in memory longer than needed | `SecretBytes` zeroizes on drop, compares in constant time, and has a redacting `Debug`; DPAPI output buffers are scrubbed before `LocalFree` |
| Tokens in logs or notifications | Logs only ever receive ids, emails, plans, and error kinds. `logging::redact` additionally masks JWT, `sk-`, `rt_`, and `at_` shaped words and control characters (log forging) at every level including debug. Notification texts come from `app::messages`, which never formats credential bytes |
| Symlink/junction redirection (write auth.json or profiles elsewhere; read a foreign file) | `WindowsFileStore` refuses reparse points on the target and its parent and opens with `FILE_FLAG_OPEN_REPARSE_POINT`, checking attributes on the opened handle. `remove_dir_all` refuses to traverse reparse points. Tested with a real junction |
| Path traversal through profile ids | Profile ids are parsed as UUIDs before they become a file name; corrupted names are listed but only deleted by exact validated id |
| auth.json readable by broad principals | The DACL is checked for Everyone / Users / Authenticated Users / Anonymous / Guests read grants (explicit, non-inherit-only ACEs), and a warning is shown in the tray |
| Corrupted or foreign auth.json | Parsing is read-only and for display; unparseable credentials are never saved as a profile or switched away from silently. Every write is read back and compared |
| Killing the wrong process (ChatGPT Desktop, PID reuse) | Package-family identity plus parent links with creation-time ordering; handles are reopened and the creation time re-verified before `TerminateProcess`. No image-name kills. External `codex.exe` clients are never terminated |
| Hot swap under a running Codex (token write-back race) | auth.json is only replaced after all Codex-owned processes are confirmed gone; recovery refuses to restore while Codex runs |
| Crash or power loss mid-switch | Journal + DPAPI backup + atomic `MoveFileExW(REPLACE_EXISTING \| WRITE_THROUGH)`; recovery at startup (see architecture.md). Property-tested |
| Login process left running / hijacked | Runs the official `codex.exe` (bundled with Desktop, or PATH / explicit setting) in a kill-on-close Job object with a timeout; `OPENAI_API_KEY` / `CODEX_API_KEY` are removed from its environment |
| Concurrent tray + CLI operations | Exclusive `switch.lock` |

## Residual risks

- Same-user malware (see the trust boundary above).
- Codex writing auth.json non-atomically while *another* client refreshes during our read. Mitigated by stopping Desktop first; external clients are warned about (or blocked by policy).
- The PATH fallback for `codex.exe` trusts the user's PATH. Set `codex_cli_path` to pin it.
