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
absent. The local qualification run passed all 112 tests with these variables
set (four optional provider or hardware checks skipped).
The rehearsal runs one matched block in detached worktrees and keeps
private evidence directories. M0 goes directly to the measurement relay.
M1 and M2 route through an authenticated CW proxy with Auto-Pilot enabled.
M2 additionally enables symbol map injection. The existing offline integration
test scripts OMP pin, mount, and unmount calls and verifies their effects on the upstream system message. A second authenticated session sharing the same database does not receive the pin.

## Evidence

Each attempt writes an agent log, a final Git patch, request records in JSON
Lines, and a terminal manifest. Request records preserve completion status,
usage, first output timing, elapsed time, and any capture error. Attempt wall time
covers workspace setup, agent execution, verification, cleanup, and evidence
summarization through the start of the final manifest write; agent-only time is
reported separately. First output includes visible text, reasoning text, or a tool call delta. Unknown cache
counts stay null. The report uses all planned attempts as the denominator for
completion and correctness rates. It reports paired M1–M0, M2–M0 and M2–M1 differences
over matched, correct runs, both overall and by task. Each metric includes its
sample count. These comparisons exclude failures, so interpret them alongside
the completion and correctness rates rather than as overall success rates.

Completed-but-incorrect answers and agent exits with complete telemetry are
recorded as outcomes; they do not stop later matched arms. Timeouts, incomplete
provider or tool telemetry, failed cleanup, and processes that survive
cancellation stop the campaign. There are no harness-level retries. The report
records the stop reason, stopped run, and number of unstarted runs.

## Proposed campaign envelope

The schedule uses all six mode orders for each of three tasks, totaling 54
attempts. It uses all six task orders and all six mode orders. Each task and each
mode occurs twice in each position. Task and mode order remain paired, so
order interactions cannot be isolated independently. Cache state is not reset or independently
verified between attempts. Report order effects and initial cache state as
uncontrolled rather than claiming cache fairness. The task caps allow up to
1,008 task requests and 21 hours of agent execution. The proposal adds four
smoke requests, 10 minutes for smoke, and 10 minutes per run for setup,
verification and cleanup. The resulting 1,012-request, 30-hour-10-minute
figure is an **admission window**, not a hard wall-clock guarantee. The relay
denies new requests after the deadline and controlled child processes are
cancelled, but blocking filesystem operations, surviving processes, and final
evidence writes can overrun it. Do not authorize the full campaign on a claim
of a strict 30-hour wall limit. Render the full proposal and digest with
`python3 -B benchmark/cw105/plan.py --campaign full`.

The separately authorized pilot plan contains one matched M0/M1/M2
`board-query` block, with up to 100 provider requests including smoke, 64 M2
virtual-memory calls, and a 9,600-second (2-hour-40-minute) request-admission
window. It is an operational qualification only: one block cannot establish
product value or isolate mode-order effects. Create its plan and digest with
`python3 -B benchmark/cw105/plan.py --campaign pilot`. The offline rehearsal
allows three requests across all modes with a 120-second admission deadline.
There is no provider charge in the rehearsal.

The intended live route is Machinist LM Studio with model
`qwen3.8-27b-ud`. Before requesting authorization, establish the exact live
envelope, reconfirm that LM Studio reports the loaded model as Local and qualify the
resource limits and server cancellation. The proposed provider charge and
cap are both $0 for the pinned local endpoint. LM Studio documents local
operation and a free local model tier. [Offline operation](https://lmstudio.ai/docs/app/offline) · [Pricing](https://lmstudio.ai/pricing). Required telemetry
includes complete request usage and wall time. Cache comparisons additionally
need provider cache observations and evidence of comparable initial state.
Missing optional data must remain visible.

## Task coverage limitation

The debugging task applies a versioned four-file seed to the frozen Kanban
revision. The browser oracle rejected the seed and accepted a reference repair
for route, response handling and default origin. The plan binds the seed hash.
The README check verifies that the override variable is named, and a reviewer
must assess whether its explanation is sufficient.

The longer cross-stack implementation has a larger budget, but meaningful
context pressure has not been established. A context-pressure comparison is
eligible only after at least three correct matched M1/M2 `board-query` pairs
show M1 peak observed context at or above 80% of the model window. If fewer
than three such pairs occur, report the result as ordinary-length coding only;
do not claim a long-context benefit. A pilot cannot meet this evidence
threshold and cannot support a product-value recommendation.

## Pre-registered decision rule

The full campaign can recommend **GO** only with at least nine correct matched
M1/M2 pairs overall and at least three per task, at least three completed M2
runs per task that actually use a virtual-memory tool, no M2 completion or
correctness regression by task, and at least 10% lower median provider-reported
input tokens on the matched correct pairs. The 10% threshold is a decision
target, not a claim of statistical significance. A long-context claim also
requires the three-pair 80% pressure criterion above; otherwise any GO is
limited to ordinary-length coding sessions.

Use **ITERATE** when safety stops or incomplete telemetry leave too few pairs,
when M2 does not use the tools enough to evaluate the treatment, or when a
correctness regression has a bounded fix to evaluate. Use **NO-GO** when a
complete full campaign meets the evidence minimums but misses the input-token
target. Latency alone cannot establish GO while cache state is uncontrolled;
local provider cost is zero and local energy is not measured, so neither is a
value signal in this experiment.

## Open qualification work

- Spring and Nuxt reference solutions passed their independent oracles. The
  board reference, including `fixtures/BoardPage.test.ts`, also passed the
  complete scorer and frozen project checks from a fresh worktree. Its API
  oracle uses 55 records to check default and maximum page sizes, continuation,
  ordering, filters, and returned task fields. The runner
  scored a submitted health fixture from a fresh frozen worktree.
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
- The request-admission deadline gates new requests, agent and verification
  subprocesses, Git operations, and daemon startup. It begins before live
  preflight; SSH preflight commands use its remaining time and hashing checks
  it between chunks. Blocking filesystem operations can still overrun the
  window. A process that does not exit after SIGKILL is reported as incomplete
  and its worktree is retained; the runner cannot guarantee that process has
  stopped. A finished run closes request admission and interrupts an in-flight
  loopback stream. Relay shutdown has bounded waits; a surviving relay is
  marked incomplete and may still alter evidence. Final evidence writes have
  no independent bound. This is explicitly not a hard wall-clock ceiling and
  remains a live-authorization blocker.
- OMP's tested JSON events identify tool starts and ends but do not include
  timestamps or durations. An identical wrapper in all three modes records
  monotonic arrival times while forwarding the original bytes. The resulting
  durations are observer measurements, subject to buffering and wrapper
  overhead; they are not provider timings or proof of causal overhead. Counts
  require a complete event log, and timing requires matched observer records.
  A missing observer record makes a successful attempt incomplete.
- M2 virtual-memory calls have an enforced ceiling of 64 calls per run, or
  1,152 across the 18 planned M2 runs. The identical OMP observer counts
  `cw_mount`, `cw_unmount`, and `cw_pin` starts and kills the agent if the
  per-run cap is exceeded. This is a separate tool-call limit; it does not
  configure CW’s named financial SpendGuard.
- The common relay uses the harness `Spend` reservation and settlement guard
  for all three modes. It does not configure CW’s named `SpendGuard`. The
  existing CLI smoke configuration requires a positive authorization and
  allows at most eight requests, so it cannot simply represent this proposed
  zero-provider-charge campaign. Reconcile and verify the requested
  SpendGuard configuration before requesting live authorization.
- The matched pilot is a separate authorization target and must be reviewed
  before any full-campaign authorization. It does not authorize the full plan.
- Initial provider cache state is unknown and is not reset or independently
  verified. Report provider-reported cached-token values when present, but
  latency and token differences remain descriptive; do not attribute them to
  Context-Watcher or claim cache fairness.
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

A read-only follow-up on 2026-11-19 found `power1_input` or `power1_average`
and `temp2_input` for all three devices, but no `energy1_input` counter. The
runner therefore enforces only the sampled instantaneous GPU wattage and
temperature ceilings; it cannot enforce a cumulative GPU-energy ceiling from
this telemetry. The 14.847 kWh figure is not an enforced cap. Whole-host energy
and electricity cost remain unmeasured.
