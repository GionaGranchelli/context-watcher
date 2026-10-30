use chrono::Utc;
use cw_core::{
    analyze_turn_from_payload,
    autopilot::{generate_ambient_savings_pulse, should_emit_savings_pulse},
    ir::{ContextTurn, MutationRule, MutationStatus, ProviderKind},
    mutation::apply_mutations_to_payload,
    optimizer::{build_optimization_plans, resolve_strategy_alias},
};
use cw_store::CwStore;
use serde_json::Value;

pub fn check_and_apply_auto_heal(
    store: &CwStore,
    session_id: &str,
    turn: &mut ContextTurn,
    payload: &Value,
    seq: u32,
    provider: ProviderKind,
    workspace_root: Option<&str>,
) -> bool {
    let ap_cfg = store.get_autopilot_config().unwrap_or_default();
    if !ap_cfg.enabled
        || !ap_cfg.auto_heal_low_health
        || payload
            .get("cw_analysis_incomplete")
            .and_then(Value::as_bool)
            == Some(true)
    {
        return false;
    }
    if turn.health_score >= ap_cfg.min_health_score {
        return false;
    }

    let strat_id = resolve_strategy_alias(&ap_cfg.heal_strategy);
    let plans = build_optimization_plans(turn);
    let chosen_plan = plans
        .iter()
        .find(|p| p.id == strat_id && !p.items.is_empty())
        .or_else(|| {
            plans
                .iter()
                .find(|p| p.id == "clean_code_state" && !p.items.is_empty())
        });
    let Some(plan) = chosen_plan else {
        return false;
    };

    let mut applied_any = false;
    for item in &plan.items {
        let rule = MutationRule {
            id: format!("ap_heal_{}", &uuid::Uuid::new_v4().to_string()[..8]),
            session_id: session_id.to_string(),
            op: item.mutation_op.clone(),
            status: MutationStatus::Committed,
            estimated_saved_tokens: item.saved_tokens.max(0),
            estimated_kv_break_tokens: item.kv_break_tokens,
            created_at: Utc::now(),
        };
        if store.save_mutation(&rule).is_ok() {
            applied_any = true;
        }
    }

    if applied_any {
        let all_mutations = store.list_mutations(session_id).unwrap_or_default();
        let mut cloned = payload.clone();
        let rep = apply_mutations_to_payload(&mut cloned, provider, &all_mutations, seq);
        if let Ok((mut updated_turn, file_states)) = analyze_turn_from_payload(
            turn.id.clone(),
            session_id.to_string(),
            turn.parent_turn_id.clone(),
            seq,
            &cloned,
            provider,
            None,
            Some(turn.token_usage.cached_tokens),
            workspace_root,
        ) {
            updated_turn.created_at = turn.created_at;
            updated_turn.token_usage.output_tokens = turn.token_usage.output_tokens;
            updated_turn.token_usage.saved_by_mutations = rep.saved_tokens;
            let _ = store.save_turn(&updated_turn, Some(payload), Some(&cloned), None);
            let _ = store.save_file_states(session_id, &file_states);

            let cur_saved = updated_turn.token_usage.saved_by_mutations as u64;
            if cur_saved > 0 {
                let last_pulse = store
                    .get_latest_pulse_for_session(session_id)
                    .ok()
                    .flatten();
                let last_saved = last_pulse.map(|p| p.cumulative_tokens_saved).unwrap_or(0);
                if should_emit_savings_pulse(cur_saved, last_saved, 100) {
                    let session = store.get_session(session_id).ok().flatten();
                    let model = session
                        .as_ref()
                        .map(|s| s.model.as_str())
                        .unwrap_or("claude-sonnet-4");
                    let pulse = generate_ambient_savings_pulse(
                        session_id,
                        &updated_turn,
                        model,
                        provider,
                        cur_saved,
                    );
                    let _ = store.record_ambient_pulse(&pulse);
                    eprintln!("{}", pulse.formatted_message);
                }
            }

            *turn = updated_turn;
        }
    }
    applied_any
}
