# Team task boundaries and idle delivery

The active team run retains direct user instructions. Each non-command wake receives those instructions together with its delegated assignment, including teammates whose task description omits the original constraints. Run identities keep these instructions separate from unrelated runs. Native slash commands retain their bare-command dispatch.

`team_send_message` accepts the exact team-local alias `leader` in addition to member slot IDs and broadcast `*`. Teammate result reports use this alias so model-generated reports do not need to reproduce a long identifier. Arbitrary misspelled IDs are still rejected.

An idle notification without a summary or files is bookkeeping: the delivery path acknowledges it before reconciling model work. Substantive reports, interruptions and shutdown messages remain actionable. Repeated task updates notify owners and downstream members only for actual ownership or unblock transitions. Optional avatar and context-reset metadata failures retain the team in the list; unavailable reset capability is disabled.

These mechanisms preserve task context and avoid empty model wakes. They do not impose a universal semantic or tool-use guarantee on arbitrary model output. Regression acceptance therefore checks actual model/tool records, the task board, mailbox acknowledgement, completed work queues and a quiet interval after the final response.
