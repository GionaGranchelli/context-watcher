# CW-104A local context-size qualification

## Coding-agent product-value status

The original CW-104A mission remains incomplete. The context-size matrix below is supplementary retrieval evidence, with tools disabled; it is not a controlled long-context coding experiment. Twelve recovered F1/F3 coding workspaces independently pass their acceptance suites and additional behavior/signature checks. Their visible prompts, starting file contents, model identity, and pin contents agree within the comparisons. Coding-task order was not fully counterbalanced and sampling fields were omitted in the original requests, limiting causal interpretation. Current paired observations establish no measurable benefit.

A corrected local coding campaign is active using explicit temperature 0 and top-p 1, fresh fixtures, and a fixed pin. Its first history calibration measured 38,012 actual task input tokens; it is retained separately from the planned approximately-32K trials. The calibration passed; the first corrected F1 pair also passed in both modes, with higher M2 time/input in that pair. Remaining controlled runs are active. Detailed evidence and limitations are in `.audit/cw104a-local-qualification/coding/PAIRED-CODING-RESULTS.md` and `OBJECTIVE-COMPLETION-AUDIT.md`. The product-value decision is **INCONCLUSIVE**, pending the controlled coding evidence.

## Local compatibility scope

The authenticated local `/v1/models` endpoint returned HTTP 200 and advertised `Qwen3.8-Flash-Next-GSQ-RCO-IQ2_XS`. Real OMP runs exercised OpenAI-compatible Chat Completions, streaming, tool calls, and tool results. This is stateless Chat Completions history, not qualification of stateful Responses requests. The alternative Anthropic-compatible endpoint was not qualified.

Server metrics report version `0.1.44.1`, a 131,072-token maximum context, prompt-processing progress, generation timing/throughput, usage, cache counters, and available GPU memory/temperature/power observations. The serving container is `strata-gfx906-iq2-xs`, based on `docker.io/mixa3607/rocm-gfx906:7.14-complete`. The model name identifies the intended IQ2_XS artifact, but loaded-weight hashes and parameter count were not independently verified. Hardware samples and container visibility do not establish exclusive GPU allocation. Evidence is in `models-endpoint.json`, `server-compatibility-metadata.json`, `container-identity.json`, and `corrected-server-metrics.jsonl` under the coding evidence directory.

## First corrected coding pair (preliminary)

The frozen coding prompts also repeat preservation constraints in the latest task message. This makes these trials a weak test of recovering constraints forgotten from early history; the pin partly duplicates a recent instruction. Prompts remain frozen, and this limitation must accompany any product-value conclusion.

F1 repetition 1 completed in both modes at approximately 32K initial coding input (M1 31,376; M2 32,358). Both final workspaces independently pass acceptance and preserve tests. M1 used 2 coding requests, 64,119 input tokens, 1,779 output tokens, and 442.4 seconds; M2 used 3 requests, 98,867 input tokens, 1,436 output tokens, and 651.3 seconds. Cached tokens were zero. In this pair, M2 used 34,748 more input tokens and took 208.9 seconds longer, with equal acceptance. One pair is insufficient to infer a general effect. Remaining counterbalanced F1/F3 runs are active under the original eight-hour deadline; the pin and tasks remain frozen. Seed was omitted, and pre-hook provider bytes were not independently captured. See `.audit/cw104a-local-qualification/coding/corrected-f1-r1-paired-result.json`.

F3 repetition 1 M1 passed final acceptance and cache edge-case checks, but its coding stage had seven dispatches: two completed, one stream failure, and four upstream errors. Five dispatches lack reported usage. Known input/output counts are lower bounds, and its 524.4-second task time is transport-affected. The container stayed up and the server recovered; the exact cause remains unresolved. Include this failed-transport attempt in the report and do not treat the pair as a clean timing/token comparison. Evidence: `.audit/cw104a-local-qualification/coding/f3-r1-m1-final-transport-result.json`.

## Result

The local OMP × contextWatcher matrix completed for short and long interactions, in M1 passthrough and M2 PinContext, at nominal 32K, 64K, 92K, and 128K sizes. Every corrected cell returned an answer containing the requested source-grounded symbols and paths. M2 treatment delivery was verified for all 11 selected post-commit provider-bound requests.

| Nominal band | Interaction | M1 task request input | M2 task request input | Acceptance | M2 treatment |
| --- | --- | ---: | ---: | --- | --- |
| 32K | Short | 33,072 | 33,130 | Pass / Pass | Verified |
| 32K | Long, final turn | 34,055 | 35,866 | Pass / Pass | Verified |
| 64K | Short | 60,816 | 60,874 | Pass / Pass | Verified |
| 64K | Long continuation | 61,193 | 61,230 | Pass / Pass | Verified |
| 92K | Short | 89,780 | 89,838 | Pass / Pass | Verified |
| 92K | Long continuation | 90,920 | 91,480 | Pass / Pass | Verified |
| 128K | Short, direct full-context request | 123,551 | 123,609 | Pass / Pass | Verified |
| 128K | Long follow-up after OMP compaction | 3,953 | 3,796 | Pass / Pass | Verified |

“Task request input” is the provider-reported input for the request that answered the acceptance task, rather than the nominal context band. The 32K, 64K, and 92K short cells first sent a corpus bootstrap request and then the acceptance request. The corrected 128K short cell instead used a 2,112-token seed before sending the full context and acceptance task together. The 128K short cell also caused OMP to send an automatic compaction request. The local Strata metrics snapshot records task plus compaction usage of 248,005 input / 2,142 output tokens for M1 and 248,221 input / 1,706 output tokens for M2. These values agree with saved M2 proxy turns and OMP compaction timestamps. The 128K long cell is the next follow-up after compaction, not a request carrying 128K of un-compacted history.

## What this establishes

- Acceptance is a narrow four-string check: the final answer must contain two expected symbol names and two repository paths. It does not establish that the answer is fully correct, that the named test passes, or that task quality is acceptable in general.
- M2’s committed rule and delivery were checked separately from the answer. Across the 11 selected post-commit M2 requests, the saved OMP transcript had no injected pin; each request matched its captured egress with exactly one pin at the tail. Transform API replay confirmed the committed rule. Ten of 11 replayed `messages` values were structurally equal to the captured egress messages; the 128K full task payload was the one shape mismatch. This is parsed JSON structure equality, not byte-for-byte equality of the complete payload. The request-to-egress capture comparison is a separate check. The proxy’s applied-mutation ID list is empty because the OMP context hook transforms before traffic reaches the proxy.
- Provider usage records reported zero cached/reused input tokens for the measured requests. A read-only Strata metrics snapshot immediately after the campaign showed `conversation_cache.enabled=false` and zero reused requests/tokens. No cache benefit was observed in this run; other cache configurations remain untested.
- PinContext is append-only here. These measurements do not establish lower token cost, lower cost per task, or product value. They also do not establish statistical reliability: this is one task family and one run per matrix cell.

## Method and limits

The interactions used actual contextWatcher repository source, with tools disabled and the same acceptance criteria in M1 and M2. The long 64K and 92K cells continue the corresponding short-session histories; treat them as progressive observations, not independent samples. The 32K long cells used four-turn sessions. The corrected 128K short design put the corpus and acceptance question into one provider request to avoid OMP compacting between setup and answer. An earlier 128K M1 attempt failed acceptance after compaction, and an earlier M2 attempt was cancelled before producing an answer; both failures remain in the raw evidence and are excluded from the corrected matrix.

Large requests exceeded the proxy’s existing 300-second default timeout. The proxy now supports a bounded `CW_UPSTREAM_TIMEOUT_SECS` override (1–3,600 seconds; default remains 300), and this isolated run used 1,800 seconds. The initial 64K M2 attempt timed out at the default and was rerun successfully with the override. The timeout adjustment is part of the working-tree diff and has a unit test.

The local Strata endpoint was used directly through the isolated OMP/CW setup. No paid OpenAI or other hosted-provider request was made. Prefill throughput was about 160 tokens per second, so the 123K-token task requests each took about 13 minutes. These results qualify this local model/configuration only. They do not establish live hosted-provider behavior, production cache behavior, improvement in task quality beyond this acceptance check, or cost savings.

## Evidence

The append-only decision trail is [`.audit/cw104a-local-qualification.tsv`](../.audit/cw104a-local-qualification.tsv). Sanitized per-cell JSON, M2 verification, a filtered Strata metrics snapshot, and compressed OMP JSONL transcripts are saved under `.audit/cw104a-local-qualification/`. Runner scripts, corpora, databases, and proxy captures remain under `/tmp/cw104a-local-qual/`; they are not committed. One earlier composite log record with escaped separators is retained as-is and superseded by later valid rows. The 92K M1 start entry also has a timestamp discrepancy; its transcript records the first user message at 01:46:51Z and first answer at 01:56:19Z. The experiment used isolated OMP profiles, a temporary HOME/data directory, a local CW daemon, and a separate SQLite store.
