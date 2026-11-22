//! Optional cognition uses actual loopback HTTP requests, never paid model APIs.
#![cfg(feature = "llm")]

#[path = "../../../tests/support/mock_llm.rs"]
mod mock_llm;

use agentsociety_cognition::llm::{HybridPolicy, LlmPolicy, LlmSettings, Provider, parse_response};
use agentsociety_core::*;
use agentsociety_world::World;
use mock_llm::{Response, Server};
use std::time::{Duration, Instant};

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn world(agents: usize) -> World {
    World::seeded(&SimulationConfig {
        agents,
        llm_agents: agents,
        policy: PolicyKind::Llm,
        ..Default::default()
    })
    .unwrap()
}

fn settings(server: &Server) -> LlmSettings {
    LlmSettings {
        base_url: server.base_url.clone(),
        timeout_ms: 2000,
        min_interval_ticks: 1,
        ..Default::default()
    }
}

fn prepare(
    runtime: &tokio::runtime::Runtime,
    policy: &mut LlmPolicy,
    world: &World,
) -> DecisionBatch {
    runtime
        .block_on(policy.prepare(world, [100, 70, 120, 400], [0; 4], [0; 4]))
        .unwrap()
}

#[test]
fn strict_response_rejects_unknown_actions_fields_negative_values_and_prose() {
    for content in [
        "not json",
        r#"{"action":{"action":"hack"},"reasoning_summary":"no"}"#,
        r#"{"action":{"action":"rest","unexpected":true},"reasoning_summary":"no"}"#,
        r#"{"action":{"action":"buy","resource":"food","quantity":-1,"limit_price":10},"reasoning_summary":"no"}"#,
        r#"{"action":{"action":"rest"},"reasoning_summary":"ok","secret":"extra"}"#,
        "```json\n{\"action\":{\"action\":\"rest\"},\"reasoning_summary\":\"ok\"}\n```",
    ] {
        assert!(
            parse_response(content).is_err(),
            "accepted invalid response: {content}"
        );
    }
    assert!(
        parse_response(
            &serde_json::json!({"action":{"action":"rest"},"reasoning_summary":"x".repeat(513)})
                .to_string()
        )
        .is_err()
    );
    assert_eq!(
        parse_response(r#"{"action":{"action":"rest"},"reasoning_summary":"Recover energy."}"#)
            .unwrap()
            .action,
        Action::Rest {}
    );
}

#[test]
fn both_provider_protocols_send_bounded_context_and_read_structured_actions() {
    for provider in [Provider::Ollama, Provider::OpenaiCompatible] {
        let server = Server::new(move |_| match provider {
            Provider::Ollama => Response::rest(),
            Provider::OpenaiCompatible => Response::openai(
                r#"{"action":{"action":"rest"},"reasoning_summary":"Recover energy."}"#,
            ),
        });
        let mut world = world(1);
        world
            .with_agent_mut(AgentId(0), |agent| {
                for sequence in 0..20 {
                    agent.memories.push_back(MemoryRecord {
                        id: MemoryId(sequence),
                        agent_id: agent.id,
                        tick: 0,
                        importance: 30,
                        content: MemoryContent::Produced {
                            resource: Resource::Food,
                            quantity: 4,
                        },
                    });
                }
                Ok(())
            })
            .unwrap();
        let mut config = settings(&server);
        config.provider = provider;
        config.memory_limit = 2;
        if provider == Provider::OpenaiCompatible {
            config.base_url.push_str("/v1");
        }
        let mut policy = LlmPolicy::new(config).unwrap();
        let batch = prepare(&runtime(), &mut policy, &world);
        assert_eq!(
            batch.decisions[&AgentId(0)].outcome,
            ExternalOutcome::Accepted
        );
        assert_eq!(
            batch.decisions[&AgentId(0)]
                .decision
                .as_ref()
                .unwrap()
                .action,
            Action::Rest {}
        );
        assert_eq!(
            (policy.usage().input_tokens, policy.usage().output_tokens),
            (100, 20)
        );
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        assert_eq!(request.body["stream"], false);
        assert!(request.authorization.is_none());
        match provider {
            Provider::Ollama => {
                assert_eq!(request.path, "/api/chat");
                assert_eq!(request.body["format"], "json");
            }
            Provider::OpenaiCompatible => {
                assert_eq!(request.path, "/v1/chat/completions");
                assert_eq!(request.body["response_format"]["type"], "json_object");
            }
        }
        let context = request.context();
        assert_eq!(context["relevant_memories"].as_array().unwrap().len(), 2);
        assert_eq!(context["target_tick"], 1);
        assert!(context.get("agents").is_none());
        assert!(context.to_string().len() < 32_768);
    }
}

#[test]
fn calls_are_concurrent_but_application_order_and_call_budgets_are_stable() {
    let server = Server::new(|request| {
        let mut response = Response::rest();
        response.delay = Duration::from_millis(if request.context()["agent_id"] == 0 {
            120
        } else {
            30
        });
        response
    });
    let mut config = settings(&server);
    config.max_concurrency = 2;
    config.max_calls_per_tick = 3;
    config.max_total_calls = 4;
    let mut policy = LlmPolicy::new(config).unwrap();
    let mut world = world(4);
    let runtime = runtime();
    let first = prepare(&runtime, &mut policy, &world);
    assert_eq!(
        first.decisions.keys().copied().collect::<Vec<_>>(),
        [AgentId(0), AgentId(1), AgentId(2), AgentId(3)]
    );
    assert_eq!(
        first.decisions[&AgentId(3)].outcome,
        ExternalOutcome::BudgetExhausted
    );
    assert_eq!(server.peak_concurrency(), 2);
    assert_eq!(server.count(), 3);
    world.advance_tick().unwrap();
    prepare(&runtime, &mut policy, &world);
    assert_eq!(server.count(), 4);
    assert_eq!(policy.usage().attempts, 4);
    world.advance_tick().unwrap();
    let exhausted = prepare(&runtime, &mut policy, &world);
    assert!(
        exhausted
            .decisions
            .values()
            .all(|d| d.outcome == ExternalOutcome::BudgetExhausted)
    );
    assert_eq!(server.count(), 4);
}

#[test]
fn a_single_deadline_cancels_requests_waiting_for_concurrency_slots() {
    let server = Server::new(|_| {
        let mut response = Response::rest();
        response.delay = Duration::from_millis(400);
        response
    });
    let mut config = settings(&server);
    config.timeout_ms = 60;
    config.max_concurrency = 1;
    let mut policy = LlmPolicy::new(config).unwrap();
    let runtime = runtime();
    let started = Instant::now();
    let batch = prepare(&runtime, &mut policy, &world(4));
    assert!(
        started.elapsed() < Duration::from_millis(350),
        "deadline became one timeout per queued agent"
    );
    assert!(
        batch
            .decisions
            .values()
            .all(|d| d.outcome == ExternalOutcome::Timeout)
    );
    assert_eq!(
        policy.usage().attempts,
        4,
        "queued attempts conservatively consume budget"
    );
    assert!(server.count() <= 1);
    std::thread::sleep(Duration::from_millis(80));
    assert!(
        server.count() <= 1,
        "queued requests must not run after the batch returned"
    );
}

#[test]
fn malformed_infeasible_oversized_and_http_failures_are_distinguished() {
    let server = Server::new(
        |request| match request.context()["agent_id"].as_u64().unwrap() {
            0 => Response::ollama("not JSON"),
            1 => Response::ollama(
                r#"{"action":{"action":"buy","resource":"food","quantity":1000000,"limit_price":1000000},"reasoning_summary":"Too expensive."}"#,
            ),
            2 => {
                let mut response = Response::rest();
                response.status = 500;
                response.body = "private upstream error body".into();
                response
            }
            _ => {
                let mut response = Response::rest();
                response.body = "x".repeat(70_000);
                response
            }
        },
    );
    let mut policy = LlmPolicy::new(settings(&server)).unwrap();
    let batch = prepare(&runtime(), &mut policy, &world(4));
    for (id, expected) in [
        (0, ExternalOutcome::InvalidResponse),
        (1, ExternalOutcome::InvalidAction),
        (2, ExternalOutcome::ProviderError),
        (3, ExternalOutcome::InvalidResponse),
    ] {
        assert_eq!(
            batch.decisions[&AgentId(id)].outcome,
            expected,
            "agent {id}: {}",
            batch.decisions[&AgentId(id)].summary
        );
        assert!(batch.decisions[&AgentId(id)].decision.is_none());
    }
    assert_eq!(
        policy.usage().usage_reports,
        2,
        "invalid model JSON still consumed reported tokens"
    );
    assert!(
        !serde_json::to_string(&batch)
            .unwrap()
            .contains("private upstream")
    );
}

#[test]
fn truncated_output_is_rejected_even_if_its_text_happens_to_be_valid_json() {
    let server = Server::new(|_| {
        let mut response =
            Response::openai(r#"{"action":{"action":"rest"},"reasoning_summary":"ok"}"#);
        let mut body: serde_json::Value = serde_json::from_str(&response.body).unwrap();
        body["choices"][0]["finish_reason"] = "length".into();
        response.body = body.to_string();
        response
    });
    let mut config = settings(&server);
    config.provider = Provider::OpenaiCompatible;
    let mut policy = LlmPolicy::new(config).unwrap();
    let batch = prepare(&runtime(), &mut policy, &world(1));
    assert_eq!(
        batch.decisions[&AgentId(0)].outcome,
        ExternalOutcome::InvalidResponse
    );
    assert_eq!(policy.usage().output_tokens, 20);
}

#[test]
fn redirects_are_not_followed_and_failures_respect_per_agent_cooldowns() {
    let redirect_target = Server::new(|_| Response::rest());
    let url = redirect_target.base_url.clone();
    let server = Server::new(move |_| {
        let mut response = Response::rest();
        response.status = 307;
        response.headers.push(("Location".into(), url.clone()));
        response
    });
    let mut config = settings(&server);
    config.min_interval_ticks = 3;
    let mut policy = LlmPolicy::new(config).unwrap();
    let mut world = world(1);
    let runtime = runtime();
    assert_eq!(
        prepare(&runtime, &mut policy, &world).decisions[&AgentId(0)].outcome,
        ExternalOutcome::ProviderError
    );
    for _ in 0..2 {
        world.advance_tick().unwrap();
        assert!(prepare(&runtime, &mut policy, &world).decisions.is_empty());
    }
    world.advance_tick().unwrap();
    prepare(&runtime, &mut policy, &world);
    assert_eq!(server.count(), 2);
    assert_eq!(redirect_target.count(), 0);
}

#[test]
fn hybrid_keeps_routine_survival_local_and_only_revisits_stalled_history() {
    let world = world(1);
    let mut agent = world.agent(AgentId(0)).unwrap().clone();
    assert!(!HybridPolicy::should_consult(&agent));
    for tick in 1..=4 {
        agent.history.push_back(ActionRecord {
            tick,
            decision: Decision {
                action: Action::Wait {},
                reason: DecisionReason::NoOpportunity,
            },
            accepted: true,
            rejection: None,
        });
    }
    assert!(HybridPolicy::should_consult(&agent));
    agent.needs.energy = 0;
    assert!(!HybridPolicy::should_consult(&agent));
    agent.needs.energy = 100;
    agent.needs.hunger = 50;
    assert!(!HybridPolicy::should_consult(&agent));
}

#[test]
fn offline_agents_and_zero_call_budget_never_contact_a_provider() {
    let server = Server::new(|_| Response::rest());
    let mut config = settings(&server);
    config.max_total_calls = 0;
    let mut policy = LlmPolicy::new(config).unwrap();
    let runtime = runtime();
    let batch = prepare(&runtime, &mut policy, &world(4));
    assert_eq!(batch.decisions.len(), 4);
    assert!(
        batch
            .decisions
            .values()
            .all(|d| d.outcome == ExternalOutcome::BudgetExhausted)
    );
    let rules = World::seeded(&SimulationConfig::default()).unwrap();
    assert!(prepare(&runtime, &mut policy, &rules).decisions.is_empty());
    assert_eq!(server.count(), 0);
    assert_eq!(policy.usage().attempts, 0);
}

#[test]
fn invalid_configuration_and_missing_credentials_fail_before_requests() {
    for url in [
        "file:///etc/config",
        "http://user:password@localhost",
        "http://localhost?key=value",
        "http://localhost/#fragment",
    ] {
        assert!(
            LlmPolicy::new(LlmSettings {
                base_url: url.into(),
                ..Default::default()
            })
            .is_err()
        );
    }
    for settings in [
        LlmSettings {
            timeout_ms: 0,
            ..Default::default()
        },
        LlmSettings {
            max_concurrency: 0,
            ..Default::default()
        },
        LlmSettings {
            max_calls_per_tick: 129,
            ..Default::default()
        },
        LlmSettings {
            min_interval_ticks: 0,
            ..Default::default()
        },
        LlmSettings {
            memory_limit: 17,
            ..Default::default()
        },
        LlmSettings {
            api_key_env: Some("AGENTSOCIETY_TEST_MISSING_KEY_582746".into()),
            ..Default::default()
        },
    ] {
        assert!(LlmPolicy::new(settings).is_err());
    }
}
