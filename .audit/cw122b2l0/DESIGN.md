# CW-122B2-L0 design decision

## Alternatives

1. Add GPT-6 Luna to `ModelPricingProfile` and teach every estimate and settlement caller about context tiers and cache-write TTL buckets.
2. Keep the existing profile API unchanged, use the dated pricing snapshot only in provider-specific SpendGuard settlement, and make the smoke runner use a separate single-attempt authorization and launcher.

## Decision

Use option 2. `ModelPricingProfile` is consumed by policy and calibration code that has neither request-level tier nor cache-write telemetry. A flat entry would imply precision those callers do not have. Luna therefore remains unavailable to legacy estimates and `record_usage`; the Responses settlement path strictly parses its full usage buckets and prices from the same dated JSON snapshot consumed by the CW-122B2-P analyzer.

The smoke is a separate execution contract: default `NO_GO`, exact model/endpoint/source/pricing binding, bounded requests and deadline, durable single-use authorization, no fallback or redirects, and private evidence. The existing 12-run authorization and CW-122A frozen files stay unchanged. The new source is stacked on PR #44 until that PR is merged and independently approved.

## Rejected alternatives

- Editing the 12-run authorization contract would conflate one diagnostic smoke with a preregistered campaign.
- Replacing the CW-122A historical freeze would erase provenance. Its old digest remains evidence of the old source and is expected to reject changed source.
- Duplicating price constants in Rust would create two pricing authorities.
- Supplying a provider secret to Codex or tool subprocesses is not accepted as a substitute for isolation. If the launch environment cannot demonstrate isolation, the real runner must refuse to start.

## Boundary

This task does not authorize an API request. Mock tests can qualify routing and gates, but cannot establish account access, billed prices, provider cache behavior, or product value. The six-pair campaign remains separately disabled.
