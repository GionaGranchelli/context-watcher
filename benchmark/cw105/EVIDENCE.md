# CW-105 evidence schema

Each attempt directory contains a manifest, agent log, final patch, and
request records. The manifest identifies the run, task, frozen repository
revision, repetition, mode, terminal status and correctness outcome.
The final patch is relative to the seeded baseline when a task has a seed.
Agent logs and patches can contain workspace data and should remain private
until reviewed.

Each admitted request record contains request and response byte counts and
SHA-256 digests, time to first output, elapsed time, completion state,
reported input and output usage, optional cached input usage, settlement and
errors. A response digest covers the received prefix when the stream ends
early. A denied request records the reason. Missing usage is null, and a
request with missing terminal usage remains incomplete.

The manifest aggregates input and output only when every request has complete
usage. Cached input remains null if any request omits it. Peak observed context
fraction is the highest reported input count divided by the configured model
context window. It does not include unobserved server overhead. Provider cost
uses the approved local provider rate. GPU samples are separate timestamped
readings and do not measure whole-host energy.

The OMP event log is the source for tool call counts and any reported
durations. Missing durations remain null. The report includes planned,
completed, correct and missing counts per arm. Matched comparisons use
correct completed M1/M2 pairs and show a sample count for every median.
The long-task pressure counter uses the proposed 80% input threshold. Cache
fairness and provider cache hits need direct provider evidence.

The current implementation stores digests rather than raw request/response
bodies. This limits wire-level auditing and replay. The live campaign remains
NO-GO until evidence retention and privacy handling are reviewed.
