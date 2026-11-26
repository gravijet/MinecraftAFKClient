@AGENTS.md

## Claude Code

- Real-protocol checks: run the official server jar with `online-mode=false` and `server-ip=127.0.0.1`, connect with `--offline <name>`. For the encrypted 26.4 login, start the server with `-Dminecraft.api.{auth,account,session,services}.host=<local stub>` and set `AFK_SESSION_SERVER` for the client. Never use customer accounts or live panel data.
- Measure memory on Linux (`/proc/<pid>/status` → `RssAnon`), not on Windows.
