# Architecture Decision Records

Technical decisions that shape turnout, in the order they were made. Format: Context / Decision / Consequences. A superseded ADR is never deleted - its status changes and it points to the successor.

| # | Title | Status |
| --- | --- | --- |
| [0001](0001-rust.md) | Rust as the implementation language | accepted |
| [0002](0002-cookie-jar-gateway.md) | Dev gateway keeps a cookie jar per app+server | accepted |
| [0003](0003-os-keyring.md) | Secrets live in the OS keyring | accepted |
| [0004](0004-no-env-file-fallback.md) | No env-file generation fallback | accepted |
| [0005](0005-no-legacy-import.md) | No import from the predecessor tool | accepted |
| [0006](0006-starlight-diataxis-docs.md) | Docs: Astro Starlight structured by Diátaxis | accepted |
| [0007](0007-json-catalogs.md) | Catalogs as one JSON file per entity kind | accepted |
| [0008](0008-state-file-over-ipc.md) | Bindings travel through the state file, not IPC | accepted |
| [0009](0009-trusted-publishing.md) | Registry publishing via OIDC trusted publishing | accepted |
| [0010](0010-entity-split.md) | Servers, credentials and paths as three entities | accepted |
| [0011](0011-russh-transport.md) | SSH on russh, and one TLS stack (rustls) | accepted |
