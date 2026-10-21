# Employee dependency distribution

The enterprise and desktop components must be upgraded together for portable custom experts. The desktop reads `GET /api/one/employee/team-sync-bundle`; an older server without this route leaves the previous local cache intact instead of replacing it with an incomplete employee list.

The bundle contains only the caller's visible employees and their referenced expert snapshots. Expert snapshots carry portable profile, rules, prompts and backend selection. They exclude the administrator's model credentials, private MCP identifiers, skills and preferences. Existing skill and MCP distribution applies its independent authorization filters.

The local `POST /api/one/employee/team-sync` request accepts `agents`, `assistants`, `sourceScope` and `authoritative`. Every referenced expert must be present in the bundle. Dependencies receive deterministic, user-scoped identities derived from the enterprise source and snapshot key. A conflicting personal definition is rejected. Unchanged definitions and employees retain their update timestamps. Personal schedules remain local.

Team expert copies are execution dependencies and do not appear in the ordinary personal expert catalog. Employee cards display their synchronized expert name. After a successful authoritative reconciliation without conflicts, obsolete team expert copies are removed, including copies from a previous enterprise endpoint. Personal experts are preserved.

At runtime, employees resolve their bound resources from the authorized local team copies. An unspecified MCP selection becomes an explicit empty selection before employee bindings are mounted. It must never inherit every enabled connector. Diagnostic logs record employee identifiers and resource counts without credentials, rules, prompts or tool payloads.

DreamEngine resolves selected skill manifests through the caller's skill catalog and passes their parsed metadata to `AgentBootstrap::host_skills`. Employee runtime resolution also passes authorized bound content through a typed, non-serialized runtime field, including enterprise registry skills that are not local skill files. This field is never decoded from `conversation.extra`. The engine uses the same metadata for its prompt index and actual `Skill` tool. This lookup does not mutate the shared ACP discovery cache. Optional unselected team skills and another user's private skills are excluded; administrator-required auto-active team skills remain enabled. Original manifest roots are retained for local relative resources and scripts. A model printing a skill name is not proof that the skill body loaded: acceptance must include a successful `Skill` tool result containing the expected body.

Both desktop and enterprise builds revalidate employee authorization on every send, including already-open conversations. Removing an employee must block its old conversation before a cached runtime or tool can execute. Changes to employee configuration, bound skill content or MCP transport require a new execution rather than silently retaining stale resources.

The employee roster searches all synchronized names, descriptions and expert display names, displays 20 employees per page, and requests run history only for the visible page.

The desktop synchronization cycle waits for both skill and MCP materialization before publishing employees. A failed dependency synchronization retains the employee cache. Cycles are serialized across context changes; obsolete cycles stop before publishing, and departure cleanup waits for already-started local writes. Employee detail history belongs to the selected employee and modal lifetime, so a late response cannot replace another employee's history.

Verification should include a bundle with distinct experts, skills and connectors; an actual model turn that reports its standing expert instructions and skill content and calls the bound MCP; connection-start counts; repeat sync; authorization removal and restoration; offline cache retention; and preservation of personal resources. A successful API response alone does not verify runtime execution.
