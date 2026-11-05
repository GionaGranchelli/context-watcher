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
- [x] Run L0 gates: workspace 312 passed/23 ignored, Clippy, format/diff, and L0 Python suite 17 passed.
- [x] Run supplemental local CI equivalents: Hermes 12; CW-104B 15 plus offline fixture pass; CW-122A 20 plus offline rehearsal pass; Web checks/tests/build; cargo audit; npm audit at the CI high threshold.
- [x] Open focused dependent review PR #45 against PR #44.
- [ ] After PR #44 is independently approved/merged, retarget PR #45 to `main`, rerun CI, and obtain independent approval.
- [x] Leave live smoke and full campaign execution disabled absent separate exact spend authorization.

Current verification: the L0 Python suite has 17 offline tests, including authorization contract consistency, credential replacement rejection, a local HTTP mock, persistent authorization claim reuse, partial-evidence retention, and resource-failure handling. Rust loopback integration tests verify the actual CW proxy, complete GPT-6 Luna usage settlement, exact request/response capture, and rejection of non-OpenAI fallback. The `codex` executable is unavailable on this host, so the actual Codex process was not qualified with this model. No real provider call or spend occurred. PR #45 is a dependent PR against #44. Only GitGuardian is green; `.github/workflows/ci.yml` limits CI to PRs targeting `main`, so CI and independent review remain pending until #44 merges and #45 is retargeted. The original CW-122A freeze is preserved byte-for-byte in this audit bundle; the current versioned freeze records the new source hashes and prior-freeze SHA. Campaign inputs and historical results remain unchanged. The CW-122A synthetic rehearsal passed with product value still NOT_EVALUATED and live execution NOT_AUTHORIZED. CW-122A Python tests used system Python with the exact pinned jsonschema because python3-venv is unavailable on this host.
