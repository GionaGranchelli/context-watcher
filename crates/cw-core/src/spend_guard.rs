//! Production budget safeguards and admission control for live provider execution.
//!
//! Enforces:
//! 1. Fail-closed explicit authorization (no live execution without authorized budget).
//! 2. Pre-session / pre-request admission reservations (fails before issuing requests if budget is exhausted).
//! 3. Real-time cost accounting from provider-reported usage.
//! 4. Per-session cost ceiling and iteration caps.
//! 5. Emergency killswitch on budget breach or unexpected cost spikes.

use crate::ir::ProviderKind;
use crate::policy::find_pricing_profile;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use thiserror::Error;

pub const MICRO_USD_PER_USD: f64 = 1_000_000.0;

#[derive(Error, Debug, Clone, PartialEq)]
pub enum SpendGuardError {
    #[error("Live execution spend is not authorized (budget is zero or unset)")]
    SpendNotAuthorized,

    #[error("Invalid spend authorization value '{raw}': expected a finite positive USD amount")]
    InvalidAuthorization { raw: String },

    #[error("Provider usage telemetry unavailable or incomplete: {reason}")]
    UsageTelemetryUnavailable { reason: String },

    #[error(
        "Budget ceiling exceeded: Committed ${spent_usd:.4} + Reserved ${reserved_usd:.4} + Requested ${requested_usd:.4} exceeds limit ${limit_usd:.4}"
    )]
    BudgetExceeded {
        spent_usd: f64,
        reserved_usd: f64,
        requested_usd: f64,
        limit_usd: f64,
    },

    #[error("Killswitch is active: {reason}")]
    KillswitchActive { reason: String },

    #[error("Session cost ceiling exceeded: ${cost_usd:.4} exceeds limit ${limit_usd:.4}")]
    SessionCostExceeded { cost_usd: f64, limit_usd: f64 },

    #[error("Session request count limit exceeded: {count} requests exceeds cap {limit}")]
    RequestLimitExceeded { count: usize, limit: usize },

    #[error("Unverified or missing pricing profile for model '{model}' on provider {provider:?}")]
    UnverifiedPricingModel {
        model: String,
        provider: ProviderKind,
    },
}

#[derive(Debug, Clone)]
pub struct SpendGuardConfig {
    /// Explicit dollar authorization ceiling (e.g. 0.25 USD). Must be > 0.0 to authorize live execution.
    pub authorized_budget_usd: f64,
    /// Default reservation per session attempt (e.g. 0.05 USD).
    pub session_reservation_usd: f64,
    /// Hard ceiling on spend for a single session (e.g. 0.15 USD).
    pub max_session_cost_usd: f64,
    /// Hard ceiling on model requests per session (e.g. 25 requests).
    pub max_requests_per_session: usize,
    /// Whether unverified models without explicit pricing profiles are rejected.
    pub require_verified_pricing: bool,
    /// Per-request admission reservation held while a provider request is in flight.
    pub request_reservation_usd: f64,
}

impl Default for SpendGuardConfig {
    fn default() -> Self {
        Self {
            authorized_budget_usd: 0.0, // Fail-closed by default
            session_reservation_usd: 0.05,
            max_session_cost_usd: 0.15,
            max_requests_per_session: 25,
            require_verified_pricing: true,
            request_reservation_usd: 0.01,
        }
    }
}

/// Parses an explicit spend authorization. Missing values are not authorized and
/// malformed, non-finite, zero or negative values are rejected; there is no default.
pub fn parse_authorization(raw: Option<&str>) -> Result<f64, SpendGuardError> {
    let Some(raw) = raw else {
        return Err(SpendGuardError::SpendNotAuthorized);
    };
    match raw.trim().parse::<f64>() {
        Ok(v) if v.is_finite() && v > 0.0 => Ok(v),
        _ => Err(SpendGuardError::InvalidAuthorization {
            raw: raw.to_string(),
        }),
    }
}

/// Provider-reported usage for one request. Input and output counts must be explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedUsage {
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub output_tokens: u64,
}

impl ObservedUsage {
    /// Parses an OpenAI Responses (`input_tokens`/`output_tokens`) or Chat Completions
    /// (`prompt_tokens`/`completion_tokens`) usage object. Missing input or output counts
    /// are an error, never zero. A missing cached-token detail is treated as zero cached
    /// tokens, which bills every input token at the (higher) uncached rate.
    pub fn from_provider_usage(usage: &Value) -> Result<Self, SpendGuardError> {
        let field = |names: &[&str]| names.iter().find_map(|n| usage.get(*n)?.as_u64());
        let input_tokens = field(&["input_tokens", "prompt_tokens"]).ok_or_else(|| {
            SpendGuardError::UsageTelemetryUnavailable {
                reason: "missing input token count".into(),
            }
        })?;
        let output_tokens = field(&["output_tokens", "completion_tokens"]).ok_or_else(|| {
            SpendGuardError::UsageTelemetryUnavailable {
                reason: "missing output token count".into(),
            }
        })?;
        let cached_input_tokens = usage
            .pointer("/input_tokens_details/cached_tokens")
            .or_else(|| usage.pointer("/prompt_tokens_details/cached_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if cached_input_tokens > input_tokens {
            return Err(SpendGuardError::UsageTelemetryUnavailable {
                reason: "cached tokens exceed input tokens".into(),
            });
        }
        Ok(Self {
            input_tokens,
            cached_input_tokens,
            output_tokens,
        })
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct SessionLedger {
    requests: usize,
    spent_micro_usd: u64,
}

/// RAII reservation guard that holds allocated funds in `reserved_micro_usd`.
/// If dropped without calling `commit()`, funds are automatically returned to available budget.
#[derive(Debug)]
pub struct BudgetReservation {
    spend_guard: Arc<SpendGuardInner>,
    reserved_micro_usd: u64,
    committed: bool,
}

impl BudgetReservation {
    pub fn reserved_usd(&self) -> f64 {
        self.reserved_micro_usd as f64 / MICRO_USD_PER_USD
    }

    /// Commit the reservation against actual reconciled cost.
    /// Deducts `reserved_micro_usd` and atomically records `actual_cost_usd` into committed spend.
    pub fn commit(mut self, actual_cost_usd: f64) -> Result<f64, SpendGuardError> {
        let res = self
            .spend_guard
            .commit_internal(self.reserved_micro_usd, actual_cost_usd);
        self.committed = true;
        res
    }
}

impl Drop for BudgetReservation {
    fn drop(&mut self) {
        if !self.committed && self.reserved_micro_usd > 0 {
            self.spend_guard
                .reserved_micro_usd
                .fetch_sub(self.reserved_micro_usd, Ordering::SeqCst);
        }
    }
}

#[derive(Debug)]
struct SpendGuardInner {
    config: SpendGuardConfig,
    spent_micro_usd: AtomicU64,
    reserved_micro_usd: AtomicU64,
    killswitch_tripped: AtomicBool,
    sessions: Mutex<HashMap<String, SessionLedger>>,
}

impl SpendGuardInner {
    fn commit_internal(
        &self,
        reserved_micro_usd: u64,
        actual_cost_usd: f64,
    ) -> Result<f64, SpendGuardError> {
        let actual_micro_usd = (actual_cost_usd * MICRO_USD_PER_USD).round() as u64;

        // Atomically unreserve
        self.reserved_micro_usd
            .fetch_sub(reserved_micro_usd, Ordering::SeqCst);

        // Atomically add actual spend
        let prev_spent = self
            .spent_micro_usd
            .fetch_add(actual_micro_usd, Ordering::SeqCst);
        let total_spent = prev_spent + actual_micro_usd;
        let auth_micro = (self.config.authorized_budget_usd * MICRO_USD_PER_USD).round() as u64;

        if total_spent > auth_micro {
            self.killswitch_tripped.store(true, Ordering::SeqCst);
            return Err(SpendGuardError::BudgetExceeded {
                spent_usd: total_spent as f64 / MICRO_USD_PER_USD,
                reserved_usd: self.reserved_micro_usd.load(Ordering::SeqCst) as f64
                    / MICRO_USD_PER_USD,
                requested_usd: actual_cost_usd,
                limit_usd: self.config.authorized_budget_usd,
            });
        }

        Ok(actual_cost_usd)
    }
}

/// Thread-safe budget safeguard engine.
#[derive(Debug, Clone)]
pub struct SpendGuard {
    inner: Arc<SpendGuardInner>,
}

impl SpendGuard {
    pub fn new(config: SpendGuardConfig) -> Self {
        Self {
            inner: Arc::new(SpendGuardInner {
                config,
                spent_micro_usd: AtomicU64::new(0),
                reserved_micro_usd: AtomicU64::new(0),
                killswitch_tripped: AtomicBool::new(false),
                sessions: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Initialize SpendGuard from environment variables `CW_LIVE_AUTH_USD` or `CW_BUDGET_LIMIT_USD`.
    /// Fails closed if neither is set or if value <= 0.0.
    pub fn from_env() -> Result<Self, SpendGuardError> {
        let raw = std::env::var("CW_LIVE_AUTH_USD")
            .or_else(|_| std::env::var("CW_BUDGET_LIMIT_USD"))
            .ok();
        let authorized = parse_authorization(raw.as_deref())?;

        let config = SpendGuardConfig {
            authorized_budget_usd: authorized,
            ..Default::default()
        };
        Ok(Self::new(config))
    }

    pub fn config(&self) -> &SpendGuardConfig {
        &self.inner.config
    }

    pub fn total_spent_usd(&self) -> f64 {
        self.inner.spent_micro_usd.load(Ordering::SeqCst) as f64 / MICRO_USD_PER_USD
    }

    pub fn total_reserved_usd(&self) -> f64 {
        self.inner.reserved_micro_usd.load(Ordering::SeqCst) as f64 / MICRO_USD_PER_USD
    }

    pub fn remaining_budget_usd(&self) -> f64 {
        let spent = self.total_spent_usd();
        let reserved = self.total_reserved_usd();
        (self.inner.config.authorized_budget_usd - (spent + reserved)).max(0.0)
    }

    pub fn is_killswitch_tripped(&self) -> bool {
        self.inner.killswitch_tripped.load(Ordering::SeqCst)
    }

    pub fn trip_killswitch(&self) {
        self.inner.killswitch_tripped.store(true, Ordering::SeqCst);
    }

    /// Reserve funds for a session or request before dispatch.
    /// Returns a `BudgetReservation` RAII guard.
    pub fn reserve(&self, estimated_cost_usd: f64) -> Result<BudgetReservation, SpendGuardError> {
        if self.inner.killswitch_tripped.load(Ordering::SeqCst) {
            return Err(SpendGuardError::KillswitchActive {
                reason: "SpendGuard emergency killswitch is active".to_string(),
            });
        }

        if self.inner.config.authorized_budget_usd <= 0.0 {
            return Err(SpendGuardError::SpendNotAuthorized);
        }

        let req_micro_usd = (estimated_cost_usd * MICRO_USD_PER_USD).round() as u64;
        let auth_micro =
            (self.inner.config.authorized_budget_usd * MICRO_USD_PER_USD).round() as u64;

        loop {
            let spent = self.inner.spent_micro_usd.load(Ordering::SeqCst);
            let reserved = self.inner.reserved_micro_usd.load(Ordering::SeqCst);
            let total = spent + reserved + req_micro_usd;

            if total > auth_micro {
                return Err(SpendGuardError::BudgetExceeded {
                    spent_usd: spent as f64 / MICRO_USD_PER_USD,
                    reserved_usd: reserved as f64 / MICRO_USD_PER_USD,
                    requested_usd: estimated_cost_usd,
                    limit_usd: self.inner.config.authorized_budget_usd,
                });
            }

            if self
                .inner
                .reserved_micro_usd
                .compare_exchange_weak(
                    reserved,
                    reserved + req_micro_usd,
                    Ordering::SeqCst,
                    Ordering::SeqCst,
                )
                .is_ok()
            {
                return Ok(BudgetReservation {
                    spend_guard: Arc::clone(&self.inner),
                    reserved_micro_usd: req_micro_usd,
                    committed: false,
                });
            }
        }
    }

    /// Convenience to reserve session admission funds.
    pub fn reserve_session(&self) -> Result<BudgetReservation, SpendGuardError> {
        self.reserve(self.inner.config.session_reservation_usd)
    }

    /// Calculate and record cost from provider-reported usage.
    pub fn record_usage(
        &self,
        uncached_in: u32,
        cached_in: u32,
        out: u32,
        provider: ProviderKind,
        model: &str,
    ) -> Result<f64, SpendGuardError> {
        if self.inner.killswitch_tripped.load(Ordering::SeqCst) {
            return Err(SpendGuardError::KillswitchActive {
                reason: "SpendGuard emergency killswitch is active".to_string(),
            });
        }

        let profile = find_pricing_profile(provider, model);
        let cost = match profile {
            Some(p) => {
                let uncached_cost = (uncached_in as f64 / 1_000_000.0) * p.uncached_input_per_mtok;
                let cached_cost = (cached_in as f64 / 1_000_000.0) * p.cached_input_read_per_mtok;
                let out_cost = (out as f64 / 1_000_000.0) * p.output_per_mtok;
                uncached_cost + cached_cost + out_cost
            }
            None => {
                if self.inner.config.require_verified_pricing {
                    return Err(SpendGuardError::UnverifiedPricingModel {
                        model: model.to_string(),
                        provider,
                    });
                }
                // Fallback zero cost if not requiring verification
                0.0
            }
        };

        let micro_usd = (cost * MICRO_USD_PER_USD).round() as u64;
        let prev = self
            .inner
            .spent_micro_usd
            .fetch_add(micro_usd, Ordering::SeqCst);
        let total = prev + micro_usd;
        let auth_micro =
            (self.inner.config.authorized_budget_usd * MICRO_USD_PER_USD).round() as u64;

        if total > auth_micro {
            self.trip_killswitch();
            return Err(SpendGuardError::BudgetExceeded {
                spent_usd: total as f64 / MICRO_USD_PER_USD,
                reserved_usd: self.total_reserved_usd(),
                requested_usd: cost,
                limit_usd: self.inner.config.authorized_budget_usd,
            });
        }

        Ok(cost)
    }

    /// Admit one provider request *before* it is dispatched. Enforces the killswitch,
    /// the per-session request cap, the per-session cost ceiling and the global budget
    /// (via an in-flight reservation). The returned permit must be settled with the
    /// provider-reported usage; dropping it unsettled fails closed.
    pub fn admit_request(&self, session_id: &str) -> Result<RequestPermit, SpendGuardError> {
        if self.is_killswitch_tripped() {
            return Err(SpendGuardError::KillswitchActive {
                reason: "SpendGuard emergency killswitch is active".to_string(),
            });
        }
        let cfg = &self.inner.config;
        let req_micro = (cfg.request_reservation_usd * MICRO_USD_PER_USD).round() as u64;
        let max_session_micro = (cfg.max_session_cost_usd * MICRO_USD_PER_USD).round() as u64;
        let mut sessions =
            self.inner
                .sessions
                .lock()
                .map_err(|_| SpendGuardError::KillswitchActive {
                    reason: "SpendGuard ledger poisoned".to_string(),
                })?;
        let ledger = sessions.entry(session_id.to_string()).or_default();
        let next = ledger.requests + 1;
        if next > cfg.max_requests_per_session {
            return Err(SpendGuardError::RequestLimitExceeded {
                count: next,
                limit: cfg.max_requests_per_session,
            });
        }
        if ledger.spent_micro_usd + req_micro > max_session_micro {
            return Err(SpendGuardError::SessionCostExceeded {
                cost_usd: (ledger.spent_micro_usd + req_micro) as f64 / MICRO_USD_PER_USD,
                limit_usd: cfg.max_session_cost_usd,
            });
        }
        let reservation = self.reserve(cfg.request_reservation_usd)?;
        ledger.requests = next;
        Ok(RequestPermit {
            guard: self.clone(),
            session_id: session_id.to_string(),
            reservation: Some(reservation),
        })
    }

    pub fn session_request_count(&self, session_id: &str) -> usize {
        self.inner
            .sessions
            .lock()
            .ok()
            .and_then(|s| s.get(session_id).map(|l| l.requests))
            .unwrap_or(0)
    }

    pub fn session_spent_usd(&self, session_id: &str) -> f64 {
        self.inner
            .sessions
            .lock()
            .ok()
            .and_then(|s| s.get(session_id).map(|l| l.spent_micro_usd))
            .unwrap_or(0) as f64
            / MICRO_USD_PER_USD
    }

    fn add_session_spend(&self, session_id: &str, cost_usd: f64) -> u64 {
        let micro = (cost_usd * MICRO_USD_PER_USD).round() as u64;
        match self.inner.sessions.lock() {
            Ok(mut s) => {
                let l = s.entry(session_id.to_string()).or_default();
                l.spent_micro_usd += micro;
                l.spent_micro_usd
            }
            Err(_) => u64::MAX,
        }
    }

    /// Verify request count against session cap.
    pub fn check_request_count(&self, count: usize) -> Result<(), SpendGuardError> {
        if count > self.inner.config.max_requests_per_session {
            return Err(SpendGuardError::RequestLimitExceeded {
                count,
                limit: self.inner.config.max_requests_per_session,
            });
        }
        Ok(())
    }

    /// Verify session cumulative cost against session ceiling.
    pub fn check_session_cost(&self, session_cost_usd: f64) -> Result<(), SpendGuardError> {
        if session_cost_usd > self.inner.config.max_session_cost_usd {
            return Err(SpendGuardError::SessionCostExceeded {
                cost_usd: session_cost_usd,
                limit_usd: self.inner.config.max_session_cost_usd,
            });
        }
        Ok(())
    }
}

/// An admitted, in-flight provider request. Holds a budget reservation until settled.
#[derive(Debug)]
pub struct RequestPermit {
    guard: SpendGuard,
    session_id: String,
    reservation: Option<BudgetReservation>,
}

impl RequestPermit {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Settle with provider-reported usage. Missing/invalid usage or unverified pricing
    /// charges the full reservation (conservative) and trips the killswitch.
    pub fn settle(
        mut self,
        usage: Option<&Value>,
        provider: ProviderKind,
        model: &str,
    ) -> Result<f64, SpendGuardError> {
        let Some(reservation) = self.reservation.take() else {
            return Ok(0.0);
        };
        let observed = match usage {
            Some(u) => ObservedUsage::from_provider_usage(u),
            None => Err(SpendGuardError::UsageTelemetryUnavailable {
                reason: "provider returned no usage object".into(),
            }),
        };
        let priced = observed.and_then(|u| {
            let p = find_pricing_profile(provider, model).ok_or_else(|| {
                SpendGuardError::UnverifiedPricingModel {
                    model: model.to_string(),
                    provider,
                }
            })?;
            let uncached = u.input_tokens - u.cached_input_tokens;
            Ok((uncached as f64 / 1e6) * p.uncached_input_per_mtok
                + (u.cached_input_tokens as f64 / 1e6) * p.cached_input_read_per_mtok
                + (u.output_tokens as f64 / 1e6) * p.output_per_mtok)
        });
        match priced {
            Ok(cost) => {
                let committed = reservation.commit(cost);
                let session_micro = self.guard.add_session_spend(&self.session_id, cost);
                let max = self.guard.config().max_session_cost_usd;
                committed?;
                if session_micro as f64 / MICRO_USD_PER_USD > max {
                    self.guard.trip_killswitch();
                    return Err(SpendGuardError::SessionCostExceeded {
                        cost_usd: session_micro as f64 / MICRO_USD_PER_USD,
                        limit_usd: max,
                    });
                }
                Ok(cost)
            }
            Err(e) => {
                let held = reservation.reserved_usd();
                let _ = reservation.commit(held);
                self.guard.add_session_spend(&self.session_id, held);
                self.guard.trip_killswitch();
                Err(e)
            }
        }
    }
}

impl Drop for RequestPermit {
    fn drop(&mut self) {
        // Unsettled permit (aborted stream, lost recorder): fail closed.
        if let Some(reservation) = self.reservation.take() {
            let held = reservation.reserved_usd();
            let _ = reservation.commit(held);
            self.guard.add_session_spend(&self.session_id, held);
            self.guard.trip_killswitch();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spend_guard_fail_closed_without_authorization() {
        let guard = SpendGuard::new(SpendGuardConfig {
            authorized_budget_usd: 0.0,
            ..Default::default()
        });
        let res = guard.reserve_session();
        assert_eq!(res.unwrap_err(), SpendGuardError::SpendNotAuthorized);
    }

    #[test]
    fn test_spend_guard_reservation_and_commit_cycle() {
        let guard = SpendGuard::new(SpendGuardConfig {
            authorized_budget_usd: 1.00,
            session_reservation_usd: 0.10,
            ..Default::default()
        });

        assert_eq!(guard.total_spent_usd(), 0.0);
        assert_eq!(guard.total_reserved_usd(), 0.0);
        assert_eq!(guard.remaining_budget_usd(), 1.00);

        let reservation = guard.reserve_session().expect("Reservation within budget");
        assert_eq!(guard.total_spent_usd(), 0.0);
        assert!((guard.total_reserved_usd() - 0.10).abs() < 1e-6);
        assert!((guard.remaining_budget_usd() - 0.90).abs() < 1e-6);

        // Commit with actual cost $0.0234
        let cost = reservation.commit(0.0234).expect("Commit succeeded");
        assert_eq!(cost, 0.0234);
        assert_eq!(guard.total_reserved_usd(), 0.0);
        assert!((guard.total_spent_usd() - 0.0234).abs() < 1e-6);
        assert!((guard.remaining_budget_usd() - 0.9766).abs() < 1e-6);
    }

    #[test]
    fn test_spend_guard_uncommitted_drop_releases_reservation() {
        let guard = SpendGuard::new(SpendGuardConfig {
            authorized_budget_usd: 0.50,
            session_reservation_usd: 0.05,
            ..Default::default()
        });

        {
            let _res = guard.reserve(0.05).unwrap();
            assert!((guard.total_reserved_usd() - 0.05).abs() < 1e-6);
        } // Drop without commit

        assert_eq!(guard.total_reserved_usd(), 0.0);
        assert_eq!(guard.total_spent_usd(), 0.0);
        assert_eq!(guard.remaining_budget_usd(), 0.50);
    }

    #[test]
    fn test_spend_guard_budget_exceeded_rejection() {
        let guard = SpendGuard::new(SpendGuardConfig {
            authorized_budget_usd: 0.10,
            session_reservation_usd: 0.06,
            ..Default::default()
        });

        let res1 = guard.reserve_session().expect("First reservation succeeds");
        assert!((guard.total_reserved_usd() - 0.06).abs() < 1e-6);

        // Second reservation of 0.06 would bring total to 0.12 > 0.10 -> rejected
        let res2 = guard.reserve_session();
        assert!(matches!(res2, Err(SpendGuardError::BudgetExceeded { .. })));

        res1.commit(0.04).unwrap();
        assert!((guard.total_spent_usd() - 0.04).abs() < 1e-6);
    }

    #[test]
    fn test_spend_guard_record_usage_and_killswitch() {
        let guard = SpendGuard::new(SpendGuardConfig {
            authorized_budget_usd: 0.05,
            ..Default::default()
        });

        // gpt-4o-mini: uncached 0.15/M, cached 0.075/M, out 0.60/M
        // 100k uncached = $0.015
        let cost = guard
            .record_usage(100_000, 0, 0, ProviderKind::OpenAI, "gpt-4o-mini")
            .expect("First usage within budget");
        assert!((cost - 0.015).abs() < 1e-5);
        assert!(!guard.is_killswitch_tripped());

        // 400k uncached = $0.060 -> cumulative $0.075 > $0.05 -> trips killswitch
        let err = guard.record_usage(400_000, 0, 0, ProviderKind::OpenAI, "gpt-4o-mini");
        assert!(matches!(err, Err(SpendGuardError::BudgetExceeded { .. })));
        assert!(guard.is_killswitch_tripped());

        // Subsequent calls reject immediately due to killswitch
        let sub = guard.reserve(0.01);
        assert!(matches!(sub, Err(SpendGuardError::KillswitchActive { .. })));
    }

    #[test]
    fn test_spend_guard_session_cost_and_request_caps() {
        let guard = SpendGuard::new(SpendGuardConfig {
            authorized_budget_usd: 1.00,
            max_session_cost_usd: 0.10,
            max_requests_per_session: 15,
            ..Default::default()
        });

        assert!(guard.check_request_count(15).is_ok());
        assert!(matches!(
            guard.check_request_count(16),
            Err(SpendGuardError::RequestLimitExceeded {
                count: 16,
                limit: 15
            })
        ));

        assert!(guard.check_session_cost(0.09).is_ok());
        assert!(matches!(
            guard.check_session_cost(0.11),
            Err(SpendGuardError::SessionCostExceeded { .. })
        ));
    }

    fn live_guard() -> SpendGuard {
        SpendGuard::new(SpendGuardConfig {
            authorized_budget_usd: 1.0,
            max_session_cost_usd: 0.05,
            max_requests_per_session: 3,
            request_reservation_usd: 0.01,
            ..Default::default()
        })
    }

    #[test]
    fn test_parse_authorization_rejects_missing_and_invalid() {
        assert_eq!(
            parse_authorization(None),
            Err(SpendGuardError::SpendNotAuthorized)
        );
        for bad in ["", "abc", "0", "-1", "NaN", "inf"] {
            assert!(matches!(
                parse_authorization(Some(bad)),
                Err(SpendGuardError::InvalidAuthorization { .. })
            ));
        }
        assert_eq!(parse_authorization(Some(" 0.25 ")), Ok(0.25));
    }

    #[test]
    fn test_observed_usage_requires_explicit_fields() {
        let ok = serde_json::json!({"input_tokens":100,"output_tokens":5,
            "input_tokens_details":{"cached_tokens":40}});
        assert_eq!(
            ObservedUsage::from_provider_usage(&ok).unwrap(),
            ObservedUsage {
                input_tokens: 100,
                cached_input_tokens: 40,
                output_tokens: 5
            }
        );
        let no_out = serde_json::json!({"input_tokens":100});
        assert!(ObservedUsage::from_provider_usage(&no_out).is_err());
        let no_in = serde_json::json!({"output_tokens":5});
        assert!(ObservedUsage::from_provider_usage(&no_in).is_err());
        let bad = serde_json::json!({"input_tokens":1,"output_tokens":1,
            "input_tokens_details":{"cached_tokens":2}});
        assert!(ObservedUsage::from_provider_usage(&bad).is_err());
    }

    #[test]
    fn test_admit_request_enforces_request_cap_before_dispatch() {
        let g = live_guard();
        let usage = serde_json::json!({"input_tokens":1000,"output_tokens":10});
        for _ in 0..3 {
            let p = g.admit_request("s").unwrap();
            p.settle(Some(&usage), ProviderKind::OpenAI, "gpt-4o-mini")
                .unwrap();
        }
        assert!(matches!(
            g.admit_request("s"),
            Err(SpendGuardError::RequestLimitExceeded { count: 4, limit: 3 })
        ));
        // Other sessions are independent.
        assert!(g.admit_request("other").is_ok_and(|p| {
            p.settle(Some(&usage), ProviderKind::OpenAI, "gpt-4o-mini")
                .is_ok()
        }));
    }

    #[test]
    fn test_admit_request_enforces_session_cost_incrementally() {
        let g = live_guard();
        // 200k uncached gpt-4o-mini = $0.03 per request.
        let usage = serde_json::json!({"input_tokens":200_000,"output_tokens":0});
        g.admit_request("s")
            .unwrap()
            .settle(Some(&usage), ProviderKind::OpenAI, "gpt-4o-mini")
            .unwrap();
        // $0.03 + $0.01 reservation <= $0.05 -> admitted; settling to $0.06 trips.
        let p = g.admit_request("s").unwrap();
        assert!(matches!(
            p.settle(Some(&usage), ProviderKind::OpenAI, "gpt-4o-mini"),
            Err(SpendGuardError::SessionCostExceeded { .. })
        ));
        assert!(g.is_killswitch_tripped());
        assert!(matches!(
            g.admit_request("s"),
            Err(SpendGuardError::KillswitchActive { .. })
        ));
    }

    #[test]
    fn test_missing_usage_fails_closed() {
        let g = live_guard();
        let p = g.admit_request("s").unwrap();
        assert!(matches!(
            p.settle(None, ProviderKind::OpenAI, "gpt-4o-mini"),
            Err(SpendGuardError::UsageTelemetryUnavailable { .. })
        ));
        assert!(g.is_killswitch_tripped());
        assert!((g.total_spent_usd() - 0.01).abs() < 1e-9);
        assert_eq!(g.total_reserved_usd(), 0.0);
    }

    #[test]
    fn test_dropped_permit_fails_closed() {
        let g = live_guard();
        drop(g.admit_request("s").unwrap());
        assert!(g.is_killswitch_tripped());
        assert!((g.total_spent_usd() - 0.01).abs() < 1e-9);
    }
}
