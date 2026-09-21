# ADR 0017: a job's output is captured, not passed through

- Status: accepted
- Date: 2026-09-21

## Context

Up to v0.18 every command turnout started for an app - `dev`, `build`, `test`, `lint`, `run`, and the build inside `deploy` - was a pass-through. The child inherited turnout's stdout and stderr and wrote straight to the terminal. That is the simplest thing a wrapper can do, and it has three costs.

**A build is two minutes of scrollback nobody reads.** The one line that mattered - the error, the bundle size, the time - went past forty seconds ago. What a person wants from `turnout build` is that it is building, and then that it finished.

**A dev server never shuts up.** Vite prints its banner, then every hot-reload, every page reload, for as long as the session lasts. The line worth seeing is the one that says the compile broke, and it arrives between two hundred that do not.

**Nothing turnout ran can be looked at afterwards.** The output existed only as pixels. Re-running a two-minute build to read an error again is the normal way of finding out what it said.

The last of these is not only a comfort problem. Background jobs (`turnout dev` detached, with `ps`/`logs`/`stop`) are the next stage, and a detached job has nowhere to write. A wrapper that inherits the terminal cannot let go of it.

## Decision

**Every command turnout starts for an app is spawned with piped stdout and stderr, read line by line, written to a log file, and only then rendered to the terminal according to a mode.**

The modes are:

- **Quiet** (`build`, `test`, `lint`, and the build step of `deploy`): a loader while it runs, one line with the elapsed time when it ends.
- **UntilReady** (`dev`, `run`): a loader until the server announces itself, then only lines that look like a problem.
- **Stream** (`-v`, and anything that is not a terminal): every line as it arrives, which is what the pass-through did.

Three things make this safe rather than merely quieter.

**A failure shows the output, on the spot.** The last forty lines go to the terminal the moment a hidden command exits non-zero, with the log file named under them. Hiding output is only acceptable when a failure un-hides it without being asked twice.

**The log is written in every mode, including `-v`.** One file per app and command, in `logs/` under the data directory, truncated by the next run of the same command. The question a job log answers is "what did the last one say", so a directory of timestamped files would grow without bound while answering a question nobody asked.

**Anything that is not a terminal streams.** A pipe, a file, a CI job: the bytes are what they always were, and the exit code still belongs to the child. Nothing that consumed turnout's output before has to change.

## Consequences

**The two pipes are read on two threads.** A child that fills stderr while turnout drains stdout would deadlock on the unread pipe otherwise. Both feed one channel, so the order the reader sees is the order the terminal would have shown.

**"Ready" is a guess, and it is allowed to be wrong.** Three shapes are recognised: Vite's `ready in`, Next's `compiled`, and - for everything else - the first line carrying an `http://` address, which is what a server prints when it starts listening. A server that fits none of them keeps its console: after ninety seconds turnout stops waiting, prints what it held back and streams from there on. A wrong guess costs a delayed handover, never a lost line.

**The quiet filter errs loud.** Every stderr line survives it; a stdout line survives if it reads like a problem. A dev server writes its failures to stderr, and the one thing that must never be swallowed is the reason a page went blank.

**`--open` hangs off the detector rather than polling.** The reader loop is the only thing in the process that knows when the server started answering, so the browser opens then - in every mode, `-v` included, because verbosity is about the console and not about whether the page opens.

**Two environment variables exist for tests only.** `TURNOUT_CONSOLE` forces a mode and `TURNOUT_READY_PATIENCE_MS` shortens the wait. The quiet modes only ever happen on a terminal and a test harness has none, so without them the hiding, the failure tail and the fallback would be argued for and never run. They are not in `--help`: `-v` is the user-facing half.

**`utils::run_in_dir` is gone.** There is one way to start a child for an app, and it is this one - a second, simpler path would be a second place for Ctrl+C handling, job objects and exit codes to drift.
