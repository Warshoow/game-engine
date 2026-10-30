# Triage Labels

The skills speak in terms of five canonical triage roles. This file maps those roles to the actual label strings used in this repo's issue tracker.

| Label in mattpocock/skills | Label in our tracker | Meaning                                  |
| -------------------------- | -------------------- | ---------------------------------------- |
| `needs-triage`             | `needs-triage`       | Maintainer needs to evaluate this issue  |
| `needs-info`               | `needs-info`         | Waiting on reporter for more information |
| `ready-for-agent`          | `ready-for-agent`    | Fully specified, ready for an AFK agent  |
| `ready-for-human`          | `ready-for-human`    | Requires human implementation            |
| `wontfix`                  | `wontfix`            | Will not be actioned                     |

The four state labels were created on GitHub on 2026-10-30 (`wontfix`, and the categories `bug` and `enhancement`, already existed). The `wayfinder:*` labels from `issue-tracker.md` are not created: make them the first time `/wayfinder` runs.

How they are applied here (Warshow arbitrates each milestone, see `CLAUDE.md`):

| Ticket | Labels |
| ------ | ------ |
| Feature not yet arbitrated or explored with Warshow (non-goals included) | `enhancement` + `needs-triage` |
| Feature waiting for a measurement before anything is decided (#40, #47) | `enhancement` + `needs-info` |
| Done or specified, waiting for Warshow to look, play or measure on Warshow's machine (#6, #51, #53) | `enhancement` + `ready-for-human` |
| Fully specified with an agent brief, an AFK agent can take it | `ready-for-agent` (none yet) |
| Options ticket of an exploration (rejected or deferred options, kept as reference, never closed) | `documentation`, no state |
| Epic | `epic`, no state |
| In-game test checklist | `qa` + `ready-for-human` |

Project label, outside the five roles:

| Label | Meaning |
| ----- | ------- |
| `qa`  | In-game test for Warshow: a checklist of points to tick after a Windows build, one ticket per feature (or per hardware, e.g. the RTX 4060). Close it when every point is ticked or each defect has its own ticket. |

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), use the corresponding label string from this table.

Edit the right-hand column to match whatever vocabulary you actually use.
