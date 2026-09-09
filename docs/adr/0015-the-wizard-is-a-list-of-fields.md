# ADR 0015: The app wizard is a list of fields, not a run of prompts

- Status: accepted
- Date: 2026-09-09

## Context

`turnout app add` and `turnout app edit` open an interactive form when they have nothing to go on. Until v0.16.1 both spelled that form out inline: a run of `Input::new()` and `Select::new()` calls in the body of each function, one after another.

The owner reported the form as awkward to use. Reading the code before touching it showed why, and it was worse than the report:

- **Half the model was unreachable.** `App` has eight fields. `add` asked for five of them; `edit` asked for four. `dist_dir`, `env_file` and `dev_port` had no interactive path at all - they existed only as flags.
- **The field changed most often was pushed away.** `edit` printed `Commands are edited with flags: turnout app edit NAME --command NAME=CMD` and moved on. The commands of an app - `dev`, `build`, `test`, `lint` - are half of what turnout does with it.
- **Detection ruled instead of proposing.** `add` showed the detected commands and asked "Use these commands?". Answering no cleared them, leaving a new app with no commands and a note about flags.
- **The two commands had already drifted apart.** They were written separately and asked for different subsets. Nothing kept them in step, so any new field would land in one of them, or neither - which is exactly what had happened three times.

The drift is the part worth designing against. Adding the three missing questions to each function would fix today's gap and leave the mechanism that produced it untouched.

There is a second constraint, and it decides the shape. **A run of `dialoguer` calls cannot be tested.** Every prompt reads a terminal; the CLI tests drive turnout as a subprocess without one, and `pick::ensure_interactive` deliberately refuses to prompt in that case. So no test can walk the form, and a gate on the form's *completeness* has nothing to hold on to as long as the form is control flow.

## Decision

**The wizard is a list of fields - data the program can read - and both commands walk it.**

```rust
const FIELDS: &[Field] = &[
    Field { name: "path", ask: ask_path },
    Field { name: "commands", ask: ask_commands },
    // ...
];
```

Each entry names the field of `App` it fills and carries the function that asks for it. `walk` applies them in order to one `App` value: `add` starts from a blank app, `edit` from the stored one with its current values as the defaults. A field is added to the form by adding a line to the list, and it is then in both commands by construction - there is no second place to forget.

Answers land in an `App` that is passed along, so a question can depend on what came before it: the variable that carries the gateway URL is only asked when a port was given, and the name it suggests depends on the path.

**The completeness gate reads the model's own source.** The test parses `pub struct App` out of `model.rs`, compares its fields against the names in `FIELDS`, and fails naming any field that is in neither the form nor a short list of deliberate exclusions (`name`, which identifies the app rather than being edited on it). Reading the source rather than restating the field list means the test cannot quietly go stale against a struct it only remembers.

Proven by mutation before being trusted: removing `dev_port` from `FIELDS` fails the test with `these fields of App are not in the wizard: ["dev_port"]`. Mutating the *model* instead does not exercise the gate - a new field breaks every `App` initializer and the compiler stops the build first, giving a red that says nothing about the form.

**Detection proposes.** `add`, and an `edit` that moves the app to another directory, read the project and fill in the roles they recognise, never overwriting a command already set by hand. The result is simply the list the user then works on - add, change the command line, rename, remove - so there is no longer an answer that leaves an app with nothing.

**Flags keep their own road.** `add` and `edit` take the same set, and a single `apply_flags` puts them onto an app for both. Scripts and CI are unaffected: with a name and a path, `add` still runs without prompting, and `edit` with any flag still edits exactly that field.

## Consequences

- A field added to `App` fails `cargo test` until it is placed in the form or explicitly excluded with a reason. The trap that produced this ADR cannot recur silently.
- The gate proves the form is *complete*, not that the prompts behave. Prompt behaviour still needs a terminal and a person; the live check before a release is a real run of both commands.
- `app` is the only catalog on this shape. `server`, `credential` and `path` have the same duplication between their `add` and `edit`, and are candidates for the same treatment - but converting them was not part of this stage, and doing it on speculation would be a redesign nobody asked for. The form proves itself on `app` first.
- The wizard's questions are now the documented surface of `app add` / `app edit`, so a change to `FIELDS` is a change to the reference page.
