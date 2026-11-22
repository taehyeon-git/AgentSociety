//! HTTP failure recovery and model result replay across the actual tick boundary.
#![cfg(feature = "llm")]

#[path = "../../../tests/support/mock_llm.rs"]
mod mock_llm;

use agentsociety_core::*;
use agentsociety_simulation::{LlmSettings, PolicyRuntime, RunCheckpoint, Simulation};
use mock_llm::{Response, Server};

#[test]
fn model_common_property_and_contracts_have_bounded_context_validated_authority_and_replay() {
    let server = Server::new(|request| {
        let context = request.context();
        let id = context["agent_id"].as_u64().unwrap();
        let tick = context["target_tick"].as_u64().unwrap();
        let operation = match (tick, id) {
            (1, 1) => Some(InstitutionAction::Found {}),
            (2, 2 | 3) => Some(InstitutionAction::Join {
                organization: OrganizationId(0),
            }),
            (3, 2) => Some(InstitutionAction::Contribute {
                organization: OrganizationId(0),
                cash: 0,
                inventory: Inventory::new([0, 4, 2, 0]),
            }),
            (3, 1) => Some(InstitutionAction::OfferContract {
                organization: Some(OrganizationId(0)),
                counterparty: Party::Agent(AgentId(5)),
                side: OrderSide::Buy,
                resource: Resource::Wood,
                quantity: 1,
                unit_price: 10,
                deliveries: 2,
                interval_ticks: 1,
            }),
            (4, 5) => Some(InstitutionAction::AcceptContract { contract_id: 0 }),
            (5, 0) => Some(InstitutionAction::Distribute {
                organization: OrganizationId(0),
                cash_per_member: 1,
                goods_per_member: 1,
            }),
            _ => None,
        };
        let action = operation.map_or(Action::Wait {}, |operation| Action::Institution {
            operation,
        });
        Response::ollama(&serde_json::json!({"action": action, "reasoning_summary": "Use the observed shared agreement."}).to_string())
    });
    let config = SimulationConfig {
        agents: 6,
        llm_agents: 6,
        policy: PolicyKind::Llm,
        organizations: OrganizationSettings {
            enabled: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut sim = Simulation::new(config.clone()).unwrap();
    let mut replay = Simulation::new(config).unwrap();
    let mut runtime = PolicyRuntime::with_llm(LlmSettings {
        base_url: server.base_url.clone(),
        max_calls_per_tick: 6,
        max_total_calls: 36,
        min_interval_ticks: 1,
        ..Default::default()
    })
    .unwrap();
    for _ in 0..6 {
        let report = runtime.step(&mut sim).unwrap();
        let events: Vec<EventRecord> =
            serde_json::from_slice(&serde_json::to_vec(&report.events).unwrap()).unwrap();
        replay.replay_tick(&events).unwrap();
    }
    assert_eq!(sim.cumulative().group_productions, 1);
    assert_eq!(sim.cumulative().contract_deliveries, 2);
    assert_eq!(runtime.usage().accepted, 35);
    assert_eq!(sim.snapshot().unwrap(), replay.snapshot().unwrap());
    for request in server.requests.lock().unwrap().iter() {
        let context = request.context();
        let institutions = &context["institutions"];
        assert!(institutions["organizations"].as_array().unwrap().len() <= 8);
        assert!(institutions["counterparties"].as_array().unwrap().len() <= 8);
        assert!(institutions["contracts"].as_array().unwrap().len() <= 8);
        assert!(context["environment"]["fiscal"].is_object());
        assert!(context.get("agents").is_none());
    }
}

#[test]
fn model_negotiation_and_cooperation_use_bounded_context_and_replay_offline() {
    let server = Server::new(|request| {
        let context = request.context();
        let id = context["agent_id"].as_u64().unwrap();
        let tick = context["target_tick"].as_u64().unwrap();
        let action = match (tick, id) {
            (1, 0) => Action::Negotiate {
                counterparty: AgentId(2),
                side: OrderSide::Buy,
                resource: Resource::Wood,
                quantity: 2,
                unit_price: 70,
            },
            (1, 1) => Action::Cooperate {
                partner: AgentId(3),
                producer_share: 2,
            },
            (2, 2) => Action::AcceptProposal { proposal_id: 0 },
            (2, 3) => Action::AcceptProposal { proposal_id: 1 },
            (3, 0) => Action::AcceptProposal { proposal_id: 99999 },
            _ => Action::Wait {},
        };
        Response::ollama(
            &serde_json::json!({"action":action,"reasoning_summary":"Use the observed agreement."})
                .to_string(),
        )
    });
    let config = SimulationConfig {
        agents: 12,
        llm_agents: 4,
        policy: PolicyKind::Llm,
        interactions: InteractionSettings {
            negotiation: true,
            cooperation: true,
            goal_review_interval: 2,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut sim = Simulation::new(config.clone()).unwrap();
    let mut replay = Simulation::new(config).unwrap();
    let mut runtime = PolicyRuntime::with_llm(LlmSettings {
        base_url: server.base_url.clone(),
        min_interval_ticks: 1,
        max_calls_per_tick: 4,
        max_total_calls: 12,
        ..Default::default()
    })
    .unwrap();
    let mut pending_checkpoint = None;
    for tick in 1..=3 {
        let report = runtime.step(&mut sim).unwrap();
        assert_eq!(
            replay.replay_tick(&report.events).unwrap().events,
            report.events
        );
        if tick == 1 {
            pending_checkpoint = Some(
                RunCheckpoint::capture(&sim, &runtime)
                    .unwrap()
                    .restore_engine()
                    .unwrap(),
            );
        } else {
            pending_checkpoint
                .as_mut()
                .unwrap()
                .replay_tick(&report.events)
                .unwrap();
        }
        if tick == 2 {
            assert_eq!(report.stats.flows.negotiated_trades, 1);
            assert_eq!(report.stats.flows.cooperations, 1);
        }
        if tick == 3 {
            assert!(report.events.iter().any(|r| matches!(&r.event,
                Event::ExternalDecisionRecorded { result } if result.agent_id == AgentId(0) && result.outcome == ExternalOutcome::InvalidAction
            )));
        }
    }
    assert_eq!(server.count(), 12);
    assert_eq!(runtime.usage().accepted, 11);
    assert_eq!(sim.snapshot().unwrap(), replay.snapshot().unwrap());
    assert_eq!(
        sim.snapshot().unwrap(),
        pending_checkpoint.unwrap().snapshot().unwrap()
    );
    for request in server.requests.lock().unwrap().iter() {
        let context = request.context();
        let social = &context["interactions"];
        assert_eq!(social["settings"]["negotiation"], true);
        assert!(social["counterparties"].as_array().unwrap().len() <= MAX_OBSERVED_PEERS);
        assert!(social["proposals"].as_array().unwrap().len() <= MAX_OPEN_PROPOSALS);
        assert!(context.get("goal_progress").is_some());
        assert!(context.get("agents").is_none());
        if context["agent_id"] == 2 && context["target_tick"] == 2 {
            assert!(
                social["proposals"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p["id"] == 0)
            );
        }
    }
}

#[test]
fn checkpoint_resume_preserves_cooldowns_and_does_not_replenish_call_budget() {
    let server = Server::new(|_| Response::rest());
    let mut original = Simulation::new(SimulationConfig {
        agents: 4,
        llm_agents: 1,
        policy: PolicyKind::Llm,
        ..Default::default()
    })
    .unwrap();
    let mut runtime = PolicyRuntime::with_llm(LlmSettings {
        base_url: server.base_url.clone(),
        min_interval_ticks: 4,
        max_total_calls: 2,
        ..Default::default()
    })
    .unwrap();
    runtime.step(&mut original).unwrap();
    runtime.step(&mut original).unwrap();
    let checkpoint = RunCheckpoint::capture(&original, &runtime).unwrap();
    let checkpoint: RunCheckpoint =
        serde_json::from_slice(&serde_json::to_vec(&checkpoint).unwrap()).unwrap();
    let mut offline = checkpoint.restore_engine().unwrap();
    let (mut restored, mut resumed_runtime) = checkpoint.restore().unwrap();
    for _ in 0..8 {
        let report = resumed_runtime.step(&mut restored).unwrap();
        offline.replay_tick(&report.events).unwrap();
    }
    assert_eq!(server.count(), 2);
    assert_eq!(resumed_runtime.usage().attempts, 2);
    assert_eq!(resumed_runtime.usage().accepted, 2);
    assert_eq!(restored.snapshot().unwrap(), offline.snapshot().unwrap());
    let checkpoint = RunCheckpoint::capture(&restored, &resumed_runtime).unwrap();
    let (mut restored, mut resumed_runtime) = checkpoint.restore().unwrap();
    for _ in 0..3 {
        resumed_runtime.step(&mut restored).unwrap();
    }
    assert_eq!(server.count(), 2);
    assert!(resumed_runtime.usage().fallbacks > 0);
}

#[test]
fn llm_results_change_actions_and_replay_exactly_without_recontacting_server() {
    let server = Server::new(|_| Response::rest());
    let config = SimulationConfig {
        agents: 10,
        llm_agents: 2,
        policy: PolicyKind::Llm,
        ..Default::default()
    };
    let mut original = Simulation::new(config.clone()).unwrap();
    let mut replay = Simulation::new(config).unwrap();
    let mut runtime = PolicyRuntime::with_llm(LlmSettings {
        base_url: server.base_url.clone(),
        min_interval_ticks: 1,
        max_total_calls: 6,
        ..Default::default()
    })
    .unwrap();
    for tick in 1..=10 {
        let report = runtime.step(&mut original).unwrap();
        let batch = DecisionBatch {
            target_tick: tick,
            decisions: report
                .events
                .iter()
                .filter_map(|record| match &record.event {
                    Event::ExternalDecisionRecorded { result } => {
                        Some((result.agent_id, result.clone()))
                    }
                    _ => None,
                })
                .collect(),
        };
        let replayed = replay.step_with_decision_batch(&batch).unwrap();
        assert_eq!(report.events, replayed.events);
        if tick <= 3 {
            assert_eq!(
                original.world().agent(AgentId(0)).unwrap().current_action,
                Action::Rest {}
            );
        }
    }
    assert_eq!(server.count(), 6);
    assert_eq!(runtime.usage().accepted, 6);
    assert_eq!(original.snapshot().unwrap(), replay.snapshot().unwrap());
}

#[test]
fn invalid_responses_fall_back_to_the_identical_offline_policy_and_preserve_money() {
    let server = Server::new(|_| Response::ollama("invalid model JSON"));
    let config = SimulationConfig {
        agents: 10,
        llm_agents: 2,
        policy: PolicyKind::Llm,
        ..Default::default()
    };
    let mut actual = Simulation::new(config.clone()).unwrap();
    let mut expected = Simulation::new(config).unwrap();
    let mut runtime = PolicyRuntime::with_llm(LlmSettings {
        base_url: server.base_url.clone(),
        min_interval_ticks: 1,
        max_total_calls: 4,
        ..Default::default()
    })
    .unwrap();
    for _ in 0..20 {
        let a = runtime.step(&mut actual).unwrap();
        let b = expected.step().unwrap();
        assert_eq!(a.stats, b.stats);
        for (one, two) in actual.world().agents().zip(expected.world().agents()) {
            assert_eq!(one.account, two.account);
            assert_eq!(one.current_action, two.current_action);
            assert_eq!(one.history, two.history);
        }
    }
    assert_eq!(runtime.usage().accepted, 0);
    assert_eq!(server.count(), 4);
    assert_eq!(actual.world().tick(), 20);
}

#[test]
fn shutting_down_a_provider_does_not_stop_the_simulation() {
    let server = Server::new(|_| Response::rest());
    let mut runtime = PolicyRuntime::with_llm(LlmSettings {
        base_url: server.base_url.clone(),
        min_interval_ticks: 1,
        timeout_ms: 100,
        ..Default::default()
    })
    .unwrap();
    let mut sim = Simulation::new(SimulationConfig {
        agents: 10,
        llm_agents: 1,
        policy: PolicyKind::Llm,
        ..Default::default()
    })
    .unwrap();
    runtime.step(&mut sim).unwrap();
    assert_eq!(runtime.usage().accepted, 1);
    drop(server);
    for _ in 0..5 {
        runtime.step(&mut sim).unwrap();
    }
    assert_eq!(sim.world().tick(), 6);
    assert_eq!(runtime.usage().fallbacks, 5);
    assert!(matches!(
        sim.world()
            .agent(AgentId(0))
            .unwrap()
            .last_external_decision
            .as_ref()
            .unwrap()
            .outcome,
        ExternalOutcome::ProviderError | ExternalOutcome::Timeout
    ));
}
