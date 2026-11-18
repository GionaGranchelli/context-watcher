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
  python3 -B -m unittest discover -s benchmark/cw105
python3 -B benchmark/cw105/offline.py \
  --repository /absolute/path/to/cw-kanban-board \
  --output /tmp/cw105-rehearsal \
  --omp /absolute/path/to/omp --cw /absolute/path/to/cw
```

The test suite skips binary checks when the corresponding environment variables
are absent. The rehearsal runs one matched block in detached worktrees and keeps
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
attempts. The task caps give a maximum of 1,008 task requests and 21 hours of
agent execution before setup, verification, or authorized smoke requests.
These are design ceilings, not an authorization envelope. The offline
rehearsal allows three requests across all modes with a 120 second admission
deadline. There is no provider charge in the rehearsal.

The intended live route is Machinist LM Studio with model
`qwen3.8-27b-ud`. Before requesting authorization, establish the exact live
envelope, verify pricing or local-only billing and resource limits, and
qualify shared SpendGuard enforcement and cancellation. Required telemetry
includes complete request usage and wall time. Cache comparisons additionally
need provider cache observations and evidence of comparable initial state.
Missing optional data must remain visible.

## Open qualification work

- Execute the task oracles against representative solutions, including browser
  behavior and launching the backend without database credentials.
- Match the built-in OMP tool allowlist across all modes before task execution. The rehearsal uses a scripted text reply and the separate binary test exercises the MCP lifecycle.
- Qualify durable admission, live authorization, resource monitoring, full
  attempt evidence, and failure handling before requesting any live access.
- Record exact binaries, endpoint identity, cache controls, and the approved
  merged revision for the separately authorized experiment.
