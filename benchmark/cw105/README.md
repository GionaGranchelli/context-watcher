# CW-105 offline rehearsal

**Readiness for live authorization: NO-GO.** The integrated rehearsal exercises
the real OMP and CW binaries against a scripted loopback provider. It assigns
no correctness or product value credit.

## Reproduce

Build CW from the revision under review. Use absolute paths to that binary,
the installed OMP binary and a local Kanban repository containing
`790d5ed1394965cd8c68ffe14bf6a8fdb7504774`. The evidence directory must be new.

```sh
CW105_CW=/absolute/path/to/cw CW105_OMP=/absolute/path/to/omp \
  CW105_KANBAN=/absolute/path/to/cw-kanban-board \
  CW105_PLAYWRIGHT=/absolute/path/to/playwright/index.mjs \
  python3 -B -m unittest discover -s benchmark/cw105
python3 -B benchmark/cw105/offline.py \
  --repository /absolute/path/to/cw-kanban-board \
  --output /tmp/cw105-rehearsal \
  --omp /absolute/path/to/omp --cw /absolute/path/to/cw
```

The test suite skips opt-in integrations when their environment variables are
absent. The local qualification run passed all 60 tests with these variables set. The rehearsal runs one matched block in detached worktrees and keeps
private evidence directories. M0 goes directly to the measurement relay.
M1 and M2 route through an authenticated CW proxy with Auto-Pilot enabled.
M2 additionally enables symbol map injection. The existing offline integration
test scripts OMP pin, mount, and unmount calls and verifies their effects on the upstream system message. A second authenticated session sharing the same database does not receive the pin.

## Evidence

Each attempt writes an agent log, a final Git patch, request records in JSON
Lines, and a terminal manifest. Request records preserve completion status,
usage, first output timing, elapsed time, and any capture error. First output
includes visible text, reasoning text, or a tool call delta. Unknown cache
counts stay null. The report counts all planned attempts and compares only
matched, correct M1 and M2 runs for each measured field.

## Proposed campaign envelope

The schedule uses all six mode orders for each of three tasks, totaling 54
attempts. It deterministically rotates the task order. This balances mode
position within each task, but task positions are not fully counterbalanced
across all six permutations. Cache state is not reset or independently
verified between attempts. Report order effects and initial cache state as
uncontrolled rather than claiming cache fairness. The task caps give a maximum of 1,008 task requests and 21 hours of
agent execution before setup, verification, or authorized smoke requests.
The machine-readable proposal also reserves four smoke requests, 10 minutes for
smoke, and 10 minutes per run for setup, verification and cleanup. This gives
proposed overall ceilings of 1,012 requests and 30 hours 10 minutes, subject
to offline verification of campaign-wide enforcement. Render the proposal
and digest with `python3 -B benchmark/cw105/plan.py`. The offline
rehearsal allows three requests across all modes with a 120 second admission
deadline. There is no provider charge in the rehearsal.

The intended live route is Machinist LM Studio with model
`qwen3.8-27b-ud`. Before requesting authorization, establish the exact live
envelope, reconfirm that LM Studio reports the loaded model as Local and qualify the
resource limits and server cancellation. The proposed provider charge and
cap are both $0 for the pinned local endpoint. LM Studio documents local
operation and a free local model tier. [Offline operation](https://lmstudio.ai/docs/app/offline) · [Pricing](https://lmstudio.ai/pricing). Required telemetry
includes complete request usage and wall time. Cache comparisons additionally
need provider cache observations and evidence of comparable initial state.
Missing optional data must remain visible.

## Open qualification work

- Spring and Nuxt reference solutions passed their independent oracles. The
  runner also scored a submitted health fixture from a fresh frozen worktree.
  Use `python3 -B benchmark/cw105/verify_task.py TASK WORKTREE EVIDENCE_DIR`
  to score a candidate. Place Nuxt worktrees on the pnpm store filesystem for
  offline dependency installation.
- The matched rehearsal uses a scripted text reply. The separate binary test verifies the shared built-in tool list and exercises the M2 MCP lifecycle.
- The guarded launcher has exercised smoke-to-scoring on loopback with real OMP
  and Spring. Approval, replay prevention, request and spend enforcement, and
  local process cancellation have deterministic tests. Live LM Studio usage
  and cancellation remain unverified.
- The same Linux Landlock TCP policy applies to the agent and its child
  processes in each mode. It permits only the assigned relay or proxy port.
  The direct Machinist completion port (1234) is denied. Landlock filters
  TCP ports rather than addresses and does not filter UDP or Unix sockets.
  Use this runner for a trusted local benchmark. It is not isolation for a
  hostile agent or third-party workspace.
- Record exact binaries, endpoint identity, cache controls, and the approved
  merged revision for the separately authorized experiment.

## Machinist read-only resource preflight

The loaded model reported a 262,144-token context and two parallel slots.
Three AMD PCI devices exposed readable GPU power, power caps, and junction
temperature through sysfs. Readings are a point-in-time inventory.

| PCI device | Power reading type | Observed idle power | Reported board cap |
| --- | --- | ---: | ---: |
| 0000:04:00.0 | average | 6 W | 182 W |
| 0000:85:00.0 | input | 19 W | 225 W |
| 0000:88:00.0 | input | 21 W | 300 W |

The three reported caps sum to 707 W. At those caps, 21 agent-hours would
correspond to 14.847 kWh of GPU energy alone. This is a scenario calculation.
It excludes CPU, cooling, idle time outside the agent budgets, PSU losses and
electricity tariffs. The actual GPU assignment, caps under load, whole-host
energy, and server cancellation behavior still need qualification. A live
authorization envelope must state an enforced limit rather than rely on this
inventory.
