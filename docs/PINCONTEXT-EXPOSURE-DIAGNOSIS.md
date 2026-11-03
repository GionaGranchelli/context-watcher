# PinContext exposure during a tool chain

The patched smoke verified a one-request reminder. It did not test a directive that remains in every later request. M2 received the explicit pin on request 0 of 6. Its write decision occurred on request 3, without that pin. The original early user constraints remained in every request.

No new inference or server access was used for this diagnosis. The source is the saved patched run, `smoke-20261103T140929Z`, its hashed provider requests and responses, its native transcripts, and its closed local CW database. The executable extraction is [`diagnose.py`](../benchmark/pin_exposure/diagnose.py). Its sanitized output is [the trace report](../.audit/pin-exposure/trace.json).

## The saved requests show exactly when exposure ends

Request numbers are zero-based. Tools in the table are requested by the response to that request.

| M2 request | Input tail | Explicit pin | Response requests | Reused input tokens |
| --- | --- | --- | --- | ---: |
| 0 | user | Present in message 2 | glob | 27,929 |
| 1 | tool | Absent | read, read | 27,943 |
| 2 | tool | Absent | read, read | 28,113 |
| 3 | tool | Absent | write | 28,346 |
| 4 | tool | Absent | grep | 28,563 |
| 5 | tool | Absent | Final answer | 30,501 |

Removing only the standalone pin block from request 0 makes its message prefix exactly equal to the first three messages of request 1. Request 1 then appends the model response and the tool result. The tool result contains no pin. All later requests also contain no explicit pin. The original native transcript contains no injected experiment block, and the rule remains committed after the run.

Explicit pin exposure is therefore 1/6 requests. Exposure at the write decision is 0/1. This is input availability, not proof of attention or instruction retention. The model still receives the original early constraints, and its earlier responses can carry consequences of the initial reminder. The trace cannot reveal an internal memory state or prove that the model forgot a requirement.

The two protected prefix messages are identical across both arms and every request. Their canonical JSON hash is recorded in the trace report. The reused-token values above are historical provider telemetry, not a new cache probe. Equal prompt bytes alone do not prove physical KV reuse.

## The overlay changes a request copy, not OMP's history

1. The native [`context` hook](../plugins/pi-opencode/index.js#L245) sends OMP's messages to the local transform endpoint. It returns the endpoint's messages for the current provider request.
2. [`api_transform_context`](../crates/cw-proxy/src/api/transform.rs#L19) reads the committed rules and transforms a JSON payload. This path does not rewrite the native transcript or save the transformed messages as the next harness history.
3. [`inject_pinned_context_at_tail`](../crates/cw-core/src/mutation.rs#L820) accepts a `messages` tail only when it is supported user text. Request 0 qualifies and receives the block.
4. OMP records its original user message, the model response, and the tool result. On the next request it sends that original history, which has no injected block.
5. The next tail is a tool result. The guard abstains, so there is no new pin. The committed rule remains active and can apply again when a later real user message becomes the tail.

The `before_provider_request` fallback and its WeakSet checks prevent duplicate processing of the same dispatch. They do not retain the directive across subsequent requests. The `contains_text` check avoids adding a duplicate block already present in a payload. It is not storage or a cross-request replay mechanism. Its broad text search also does not establish an owned user-message anchor. Future exposure checks must record the exact role, location, and before/after difference.

The engine does not actively strip a historical pin. If an input already contains the block in an earlier user message and ends in a tool result, the current transform preserves that input unchanged and reports no new application. The actual OMP source history simply does not contain that block. This distinction also means `applied_rules` is not an exposure counter. An idempotent retry can report the rule applied without adding another block, while a tool-tail request can contain a previously supplied user pin without reporting a new application.

The disappearance occurs before Strata receives the next JSON request. A cache setting or a network failure cannot explain an instruction that is already absent from that captured request. There is no evidence that parked KV state supplies the missing directive as extra conversation history.

## The correctness fix has its own review

[PR #33](https://github.com/GionaGranchelli/context-watcher/pull/33) extracts the guard and tests from PR #32 onto `main`. The initial extraction contained two files and 60 added lines. It now also includes a focused endpoint regression, for three files and 131 added lines, with no benchmark runner or evidence bundle. The isolated checkout passed 83 `cw-core` tests, core Clippy with warnings denied, and formatting. GitHub CI also passed for that isolated branch. No merge occurred.

The review covered the only call to the injection helper, committed-rule filtering, whole-payload eligibility, signed-content abstention, the transform endpoint, native hook fallback, and positive and negative tests. Within that narrow scope, no additional blocker was found. The guard prevents the observed provider `tool` and native `toolResult` mutations while retaining user-string and user-text-array injection. The old deduplication test now first asserts tool-tail abstention, then tests deduplication after a real user continuation.

This review does not qualify persistence, Gemini mutation, provider-owned Responses history, or product value. Stateless tool-tail abstention currently has no dedicated skip-reason field in this API response. Measurements must inspect the full outgoing payload rather than interpret an empty or nonempty rule list as treatment exposure.

## Persistence options have different contracts

| Design | Tool and transcript integrity | Prefix consequences | Decision |
| --- | --- | --- | --- |
| Current supported user-tail overlay | Preserves tool messages and original history. Reappears on a later eligible user turn. | The protected early prefix stays unchanged. The initial reminder disappears on the first tool follow-up, changing the suffix once. | Use only for an explicitly named one-shot planning hypothesis. |
| Fixed user-message anchor in an outbound copy | Could repeat one stable overlay on the same eligible user message on every request. The tool suffix and original transcript need no edits. | If installed from the first treatment request outside the protected prefix, the treatment's own augmented prefix can remain stable. Inserting it into an already cached historical message still invalidates the suffix from that point. | Smallest persistence candidate for full-history stateless Chat. Requires design review and new qualification. It is not implemented. |
| One persistent extra user message maintained in outbound copies | Could preserve tool bytes and transcripts, but adds conversation structure and needs placement and pairing validation. | Can be stable only if the same message remains at the same position. A moving insertion changes the suffix repeatedly. | More adapter and protocol work than the fixed-anchor candidate. Do not add it for the next one-shot pilot. |
| A fresh reminder after each tool round | Avoids editing tool text only by adding new messages. Can disturb tool-result grouping or alter conversation semantics. | A reminder that moves to the new tail does not preserve the previously sent full suffix. | Reject as an automatic cache-safe repair. |
| Edit the system prefix, tool output, or native transcript | Violates at least one boundary in this study. A system insertion also changes instruction authority. | Changes a shared prefix or the canonical history. | No-go. |

A fixed anchor would need an authenticated session, a frozen rule version, and a content/structural identity for the original user message. An array index alone is insufficient. It must abstain if the anchor is missing, duplicated, outside the permitted region, signed, opaque, or covered by a provider-owned conversation. Eligibility must examine the entire request, not only an extracted prefix. A rule update, compaction, or changed anchor needs an explicit invalidation decision. Resume and retry must not duplicate or move the overlay.

This is a wire-copy design argument, not a cache qualification. The lifecycle test proves that an existing user pin can remain in a tool-tail payload without editing the tool result. It does not implement fixed-anchor replay or prove that Strata would reuse its tokens. Historical mutation of stateful Responses is outside the proposal.

## Test the one-shot hypothesis before building persistence

The current treatment can plausibly help when the first model response makes a consequential plan using early constraints. It can also affect later responses through the generated plan. It is less directly suited to an invariant first needed after a long tool chain, because the explicit reminder is absent at that decision. Neither mechanism has demonstrated product benefit here.

The F3 smoke was weak for product discrimination. The pin repeated signature and protected-file constraints, while the requested change naturally belonged in the editable cache file. Both arms passed all checks. Its slower M2 result cannot distinguish useful instruction retention from ordinary variation, and an append-only reminder cannot mechanically remove context.

The next proposal therefore tests a one-shot reminder on a harder, realistic retention task. It does not first add a persistence mechanism. The [benchmark proposal](PINCONTEXT-RETENTION-BENCHMARK.md) defines the early constraint, competing lower-trust information, independent checks, and stop criteria. If later evidence specifically supports ongoing exposure, the fixed-anchor candidate can receive a separate review.

## Reproduce the offline checks

Run the hermetic API lifecycle test:

```sh
cargo test -p cw-proxy --test pin_lifecycle
```

The test calls the actual authenticated API router and core mutation engine in-process. It opens no socket and uses no provider, OMP binary, actual credentials, or user configuration. It verifies an initial pin, idempotent retry, eight unchanged tool continuations, later-user reapplication, an unchanged source history and protected prefix, no stored turn write, and the still-committed rule. A second case verifies the already-present-pin counterfactual without claiming a new application.

Inspect the retained real trace without any network call:

```sh
python3 -B benchmark/pin_exposure/diagnose.py --output .audit/pin-exposure/trace.json
```

That command requires the owner's private raw evidence vault. `--raw-dir` accepts a securely supplied copy. Missing or mismatched evidence fails the check. Reviewers without those files can run the hermetic API test and inspect the sanitized trace and source hashes, but cannot independently rehash the private wire captures. The same diagnostic rejects the earlier tool-pinned run rather than treating it as equivalent exposure.

## Standalone correctness review follow-up

The endpoint regression in `cw-proxy/src/lib.rs` registers a committed rule and sends native OMP `toolCall` and `toolResult` shapes through the actual authenticated transform router. It asserts the complete response, including all original message fields, `status: ok`, an empty applied-rule list, and zero saved tokens. The test failed against the pre-fix core because the API appended a block to the tool result, then passed with the existing guard restored. No new production guard logic was added. The updated PR passed CI and is ready for independent human review, with no approval or merge performed.

The separate Gemini contents-tail vulnerability is tracked in [#35](https://github.com/GionaGranchelli/context-watcher/issues/35), and empty user-text eligibility in [#36](https://github.com/GionaGranchelli/context-watcher/issues/36). Neither broadens PR #33. After a human merges that PR, reconcile PR #32's overlapping correctness patch against the reviewed main branch. No merge or history rewrite has been performed. Evidence is in `.audit/pin-exposure/pr33-review-response.json`.
