# Project development rules

Read `docs/project-overview.md` before adding a feature or changing persisted data.

- Dependency direction is `desktop/server -> planner-domain`. The domain crate must stay free of UI, network and database dependencies. The desktop and server must never import one another.
- Domain model and scheduling behavior have one implementation in `crates/domain`. Do not duplicate model structs or use relative-path source includes.
- Keep GPUI state and rendering under `src/app/`; keep SQLite access in `src/core/storage.rs` and HTTP sync in `src/core/cloud.rs`.
- A background sync may mutate only its snapshot. Merge the result into the current live configuration on the UI thread, then save that live configuration.
- Every desktop snapshot sync sends its last acknowledged revision. The server compares and increments it in one transaction; never accept a stale snapshot or silently turn a conflict into a retry with an old body.
- For persisted field changes, update `tools/schema_contract.json` deliberately and add a compatibility fixture covering old data. Never rely on a field rename without migration.
- For a new video source, update acquisition, stored source tag, desktop display/link behavior, robot cards and source fixtures. For a new notification channel, update delivery, retry, binding and tests.
- Run `bash tools/check.sh` before considering work complete. Do not weaken architecture checks to make an unrelated feature pass; split the module or document and review a changed boundary.
