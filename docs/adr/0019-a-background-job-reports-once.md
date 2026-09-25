# ADR 0019: a background job reports once, keeps its failure, and can be the default

- Status: accepted
- Date: 2026-09-25

## Context

v0.20.0 put jobs in the background, and their outcome in `ps` - where it waited for somebody to think of looking. v0.21.0 makes the outcome come to the user: a desktop notification when a background job is done, a way to reach its output from there, and a way to make the background the default instead of a flag to remember.

Four questions had to be answered: who sends the notification and when, how a click reaches its target on each platform, how a failure's output survives the next run, and where "always in the background" is configured.

## Decision

**The supervisor sends it, once per outcome.** Only background jobs notify - a job in a terminal speaks there. The supervisor sends one notification when a server comes up (from the ready detector, the moment it fires), and one when the job ends by itself: finished, failed, could not start, or - for a server that had come up - stopped. A job ended by `stop` sends nothing: the supervisor dies with it. No progress notifications.

**Every notification leads somewhere.** A server's to its address, a deploy's to the stand (`--link`, passed by `deploy` to the supervisor), a failure's to the kept log, anything else to its log. Buttons offer the neighbouring places: the address, the log, the project folder.

**Windows: WinRT directly, with protocol activation.** The toast XML is turnout's own, sent through `ToastNotificationManager` from the `windows` crate. The click and the buttons are `activationType="protocol"` with `http:` or `file:` URLs, so the shell opens them - after turnout exited, from the notification centre, hours later. The sender is `lacodda.turnout`, registered per user under `HKCU\Software\Classes\AppUserModelId` (`DisplayName`, `IconUri`) on every notification. notify-rust (through tauri-winrt-notification) only offers an in-process activation callback, which a process that already exited never gets; win-toast-notify supports protocol activation but runs PowerShell for every toast and sends as "Windows PowerShell".

**Linux: notify-rust with actions; the supervisor lingers.** A click comes back over D-Bus to the process that showed the notification, so the supervisor stays up to 15 minutes after its job, answering clicks with `xdg-open`, then exits.

**macOS: notify-rust, display only.** NSUserNotificationCenter reports clicks only to an application's main run loop. The notification text names the command that shows the log.

**A failure's log is copied to `failed/` beside the log.** At the end of a failed run - not one interrupted with Ctrl+C - `logs/APP.COMMAND.log` is copied to `logs/failed/APP.COMMAND.log`. A copy, not a move, so a `logs -f` in progress reaches the end of the file it follows. A directory rather than a suffix, because command names may contain dots. `logs --failed` prints the latest kept failure.

**The supervisor's own troubles go into the job's log**, as `turnout:` lines: a command that could not be spawned, a notification that could not be shown. It has no stderr anybody reads.

**`TURNOUT_DETACH` names the commands that go to the background by default** - `all`, or command names. An environment variable, like every other environment-level setting of turnout (`TURNOUT_LOG_DIR`, `TURNOUT_UPDATE_CHECK`), not a settings file of its own. It applies only where the console would be quiet: `--foreground` and `-v` keep a run here, and so does any non-terminal stdout, so scripts and CI never have a build detached from under the deploy that follows it. A name no app has is reported.

## Consequences

**The click works where the user is.** On Windows, where the owner works, a notification read an hour later still opens the page or the log. The platform table in the docs says plainly where it does not.

**A `.log` opens like a double-click.** Windows hands a `file:` URL to whatever the association says; with no program chosen for `.log`, Windows asks - the same question a double-click in Explorer asks.

**A retry never erases the failure it follows.** The notification pointed at the kept copy, and the copy stays until the same job fails again. One copy per job slot, bounded like the logs.

**turnout writes one registry key on Windows**, for its own sender id only. Removing it is harmless: the next notification writes it again.

**`-v` means "here".** Streaming the output in full needs a terminal to stream to, so it outranks the preference - including under `TURNOUT_CONSOLE`, which forces a console mode for tests.

**Tests never touch the desktop.** `TURNOUT_TOASTS` sends notifications to a file as JSON lines; the integration suite sets it for every command and asserts on what a job sent and where it leads.
