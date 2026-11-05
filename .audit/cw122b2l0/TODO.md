# CW-122B2-L0 work list

- [x] Read the Principles section of poteto-mode.
- [x] Ground SpendGuard pricing and the Codex/proxy evidence flow.
- [x] Sketch two pricing/runner shapes and compare them against the safety rubric.
- [x] Select one design and record the rationale.
- [x] Implement exact GPT-6 Luna settlement with strict usage validation.
- [x] Add a separate NO-GO, single-use smoke authorization contract and validation-only CLI.
- [x] Build the isolated live runner; execution remains disabled without a valid separate authorization.
- [x] Add mock coverage for authorization expiry/mismatch/reuse, redirects, model mismatch, credential fallback boundaries, missing/partial telemetry, and resource failure.
- [x] Verify private evidence path permissions, ancestor symlink rejection, and artifact hashes.
- [x] Run all deterministic gates: workspace tests 311 passed/23 ignored, Clippy passed, Python suite 17 passed, format and diff checks passed.
- [ ] Open the focused review PR after PR #44 is independently approved/merged and this work is rebased onto approved `main`.
- [x] Leave live smoke and full campaign execution disabled absent separate exact spend authorization.

Current verification: the L0 Python suite has 17 offline tests, including authorization contract consistency, credential replacement rejection, a local HTTP mock, persistent authorization claim reuse, partial-evidence retention, and resource-failure handling. A Rust loopback integration test verifies the actual CW proxy, complete GPT-6 Luna usage settlement, and exact request/response capture. The `codex` executable is unavailable on this host, so the actual Codex process was not qualified with this model. No real provider call or spend occurred. `benchmark/cw122a/freeze.json` is intentionally unchanged; its historical `spend_guard.rs` digest rejects this source change as a CW-122A campaign input. PR #44 remains open, so this branch is still stacked on unapproved CW-122B2-P rather than the requested approved `main`; do not claim a main-based PR until #44 is merged.
