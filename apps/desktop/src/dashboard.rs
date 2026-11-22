use crate::files::{FilePicker, Purpose};
use crate::worker::{Command, View, Worker};
use agentsociety_core::*;
use agentsociety_memory::{MemoryQuery, recall};
use eframe::egui::{self, Color32, Pos2, RichText, Sense, Stroke, Vec2};
use egui_plot::{Line, Plot, PlotPoints};

/// A native dashboard backed exclusively by published engine state.
pub struct Dashboard {
    worker: Worker,
    selected: Option<AgentId>,
    population: usize,
    seed: u64,
    interactions: InteractionSettings,
    organizations: OrganizationSettings,
    scenario: ScenarioConfig,
    picker: Option<FilePicker>,
    file_directory: std::path::PathBuf,
    generation: u64,
    #[cfg(test)]
    controls: std::collections::BTreeMap<String, egui::Rect>,
    show_network: bool,
    selected_events_only: bool,
    event_filter: usize,
    memory_filter: Option<MemoryKind>,
    error: Option<String>,
    exit_after: Option<u64>,
}

impl Dashboard {
    /// Create a dashboard; the world starts paused unless explicitly requested.
    pub fn new(
        config: SimulationConfig,
        auto_start: bool,
        exit_after: Option<u64>,
    ) -> anyhow::Result<Self> {
        Self::with_policy(
            config,
            auto_start,
            exit_after,
            agentsociety_simulation::PolicyRuntime::default(),
        )
    }

    /// Build a dashboard whose worker owns the supplied optional cognition runtime.
    pub fn with_policy(
        config: SimulationConfig,
        auto_start: bool,
        exit_after: Option<u64>,
        policy: agentsociety_simulation::PolicyRuntime,
    ) -> anyhow::Result<Self> {
        let worker = Worker::with_policy(config.clone(), policy)?;
        if auto_start {
            worker.send(Command::Running(true))?;
        }
        Ok(Self {
            worker,
            selected: (config.agents > 0).then_some(AgentId(0)),
            population: config.agents,
            seed: config.seed,
            interactions: config.interactions.clone(),
            organizations: config.organizations.clone(),
            scenario: config.scenario.clone(),
            picker: None,
            file_directory: std::env::current_dir()?,
            generation: 0,
            #[cfg(test)]
            controls: Default::default(),
            show_network: false,
            selected_events_only: false,
            event_filter: 0,
            memory_filter: None,
            error: None,
            exit_after,
        })
    }

    fn command(&mut self, command: Command) {
        if let Err(error) = self.worker.send(command) {
            self.error = Some(error.to_string());
        }
    }

    fn button(&mut self, ui: &mut egui::Ui, label: &str) -> bool {
        let response = ui.button(label);
        #[cfg(test)]
        self.controls.insert(label.into(), response.rect);
        response.clicked()
    }

    fn pick(&mut self, purpose: Purpose, tick: Tick) {
        self.command(Command::Running(false));
        self.picker = Some(FilePicker::new(
            purpose,
            self.file_directory.clone(),
            if purpose == Purpose::Save {
                format!("checkpoint-t{tick}.json")
            } else {
                String::new()
            },
        ));
        self.error = None;
    }

    fn controls(&mut self, ui: &mut egui::Ui, view: &View) {
        ui.horizontal_wrapped(|ui| {
            ui.heading("AgentSociety");
            ui.separator();
            ui.strong(format!("Tick {}", view.snapshot.world.tick));
            if self.button(ui, if view.running { "Pause" } else { "Run" }) {
                self.command(Command::Running(!view.running));
            }
            if self.button(ui, "Step") {
                self.command(Command::Step);
            }
            ui.label("Speed");
            let mut speed = view.speed;
            if ui
                .add(egui::Slider::new(&mut speed, 1..=240).suffix(" ticks/s"))
                .changed()
            {
                self.command(Command::Speed(speed));
            }
            ui.label("Agents");
            ui.add(egui::DragValue::new(&mut self.population).range(0..=10_000));
            ui.label("Seed");
            ui.add(egui::DragValue::new(&mut self.seed));
            if self.button(ui, "Restart") {
                self.command(Command::Restart(SimulationConfig {
                    agents: self.population,
                    seed: self.seed,
                    interactions: self.interactions.clone(),
                    organizations: self.organizations.clone(),
                    scenario: self.scenario.clone(),
                    ..view.snapshot.config.clone()
                }));
                self.selected = (self.population > 0).then_some(AgentId(0));
            }
        });
        ui.horizontal_wrapped(|ui| {
            if self.button(ui, "Save checkpoint...") {
                self.pick(Purpose::Save, view.snapshot.world.tick);
            }
            if self.button(ui, "Load run...") {
                self.pick(Purpose::Load, view.snapshot.world.tick);
            }
            if self.button(ui, "Restart scenario...") {
                self.pick(Purpose::Scenario, view.snapshot.world.tick);
            }
            if self.button(ui, "Branch scenario...") {
                self.pick(Purpose::Branch, view.snapshot.world.tick);
            }
            ui.label(format!("Scenario: {}", view.snapshot.config.scenario.name));
        });
        let behavior = ui.collapsing("Social behavior (apply with Restart)", |ui| {
            ui.horizontal_wrapped(|ui| {
                let negotiation =
                    ui.checkbox(&mut self.interactions.negotiation, "Price negotiation");
                let cooperation =
                    ui.checkbox(&mut self.interactions.cooperation, "Joint production");
                let organizations = ui.checkbox(
                    &mut self.organizations.enabled,
                    "Organizations and contracts",
                );
                #[cfg(test)]
                {
                    self.controls
                        .insert("Price negotiation".into(), negotiation.rect);
                    self.controls
                        .insert("Joint production".into(), cooperation.rect);
                    self.controls
                        .insert("Organizations and contracts".into(), organizations.rect);
                }
                let _ = (negotiation, cooperation, organizations);
                ui.label("Review goals every");
                ui.add(
                    egui::DragValue::new(&mut self.interactions.goal_review_interval)
                        .range(0..=1_000_000),
                );
                ui.label("ticks (0 = off)");
            });
            if self.organizations.enabled {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Maximum members");
                    ui.add(egui::DragValue::new(&mut self.organizations.max_members).range(3..=16));
                    ui.label("Membership stake");
                    ui.add(
                        egui::DragValue::new(&mut self.organizations.membership_stake)
                            .range(1..=1_000_000),
                    );
                    ui.label("Production interval");
                    ui.add(
                        egui::DragValue::new(&mut self.organizations.production_interval)
                            .range(1..=64),
                    );
                });
            }
            ui.horizontal(|ui| {
                ui.label(format!(
                    "Restart schedule: {} ({} interventions)",
                    self.scenario.name,
                    self.scenario.events.len()
                ));
                if self.button(ui, "Use baseline") {
                    self.scenario = ScenarioConfig::default();
                }
            });
        });
        #[cfg(test)]
        self.controls
            .insert("Behavior settings".into(), behavior.header_response.rect);
        let _ = behavior;
        ui.horizontal_wrapped(|ui| {
            ui.label(&view.status);
            ui.separator();
            ui.label(format!("Engine {:.2} ms/tick", view.tick_ms));
            if view.snapshot.config.policy != PolicyKind::RuleBased {
                ui.label(format!(
                    "LLM calls {} | accepted {} | fallbacks {}",
                    view.llm_usage.attempts, view.llm_usage.accepted, view.llm_usage.fallbacks
                ));
            }
            if let Some(error) = &self.error {
                ui.colored_label(Color32::LIGHT_RED, error);
            }
        });
    }

    fn inspector(&mut self, ui: &mut egui::Ui, view: &View) {
        ui.heading("Agent inspector");
        ui.horizontal(|ui| {
            ui.label("Agent ID");
            let mut id = self.selected.map_or(0, |id| id.0);
            if ui
                .add(
                    egui::DragValue::new(&mut id)
                        .range(0..=view.snapshot.world.agents.len().saturating_sub(1) as u64),
                )
                .changed()
            {
                self.selected = Some(AgentId(id));
            }
        });
        let Some(agent) = view
            .snapshot
            .world
            .agents
            .iter()
            .find(|a| Some(a.id) == self.selected)
        else {
            ui.label("Select an agent on the map.");
            return;
        };
        ui.label(
            RichText::new(&agent.name)
                .size(19.0)
                .color(job_color(agent.occupation)),
        );
        ui.label(format!("{:?} | age {}", agent.occupation, agent.age));
        ui.label(format!("Goal: {:?}", agent.goal));
        if view.snapshot.config.interactions.goal_review_interval > 0 {
            ui.small(format!(
                "Goal reviewed at t{}; last changed at t{}",
                agent.goal_progress.last_review, agent.goal_progress.last_change
            ));
        }
        ui.label(format!("Policy: {:?}", agent.policy));
        if let Some(result) = &agent.last_external_decision {
            ui.collapsing(
                format!(
                    "AI decision (t{}, {:?})",
                    result.target_tick, result.outcome
                ),
                |ui| {
                    ui.label(format!("{} / {}", result.provider, result.model));
                    ui.label(&result.summary);
                    if let Some(decision) = &result.decision {
                        ui.label(format!("{:?}", decision.action));
                    }
                },
            );
        }
        ui.label(format!("Wallet: {} minor units", agent.account.wallet()));
        if agent.account.reserved_cash() > 0 {
            ui.label(format!(
                "Available {} / reserved {}",
                agent.account.available_cash(),
                agent.account.reserved_cash()
            ));
        }
        let wealth = u128::from(agent.account.wallet())
            + Resource::ALL
                .iter()
                .map(|&r| {
                    u128::from(agent.account.inventory().get(r))
                        * u128::from(view.snapshot.stats.prices[r.index()])
                })
                .sum::<u128>();
        ui.label(format!("Wealth: {wealth}"));
        ui.label(format!(
            "Receipts {} / spending {}",
            agent.account.earned(),
            agent.account.spent()
        ));
        ui.add(
            egui::ProgressBar::new(f32::from(agent.needs.hunger) / 100.0)
                .text(format!("Hunger {}", agent.needs.hunger)),
        );
        ui.add(
            egui::ProgressBar::new(f32::from(agent.needs.energy) / 100.0)
                .text(format!("Energy {}", agent.needs.energy)),
        );
        ui.add(
            egui::ProgressBar::new(f32::from(agent.needs.comfort) / 100.0)
                .text(format!("Comfort {}", agent.needs.comfort)),
        );
        ui.separator();
        egui::Grid::new("inventory")
            .num_columns(2)
            .striped(true)
            .show(ui, |ui| {
                for r in Resource::ALL {
                    ui.label(r.to_string());
                    let reserved = agent.account.reserved_inventory().get(r);
                    ui.label(if reserved > 0 {
                        format!(
                            "{} ({} reserved)",
                            agent.account.inventory().get(r),
                            reserved
                        )
                    } else {
                        agent.account.inventory().get(r).to_string()
                    });
                    ui.end_row();
                }
            });
        ui.separator();
        ui.label(format!("Action: {:?}", agent.current_action));
        let group = view
            .snapshot
            .institutions
            .organizations
            .iter()
            .find(|g| g.members.contains(&agent.id));
        if let Some(group) = group {
            let wealth = group.account.wealth(&view.snapshot.stats.prices);
            let index = group
                .members
                .iter()
                .position(|&id| id == agent.id)
                .unwrap_or(0) as u128;
            let count = group.members.len() as u128;
            ui.collapsing(
                format!(
                    "Cooperative #{} ({} members)",
                    group.id.0,
                    group.members.len()
                ),
                |ui| {
                    ui.label(format!(
                        "Leader: Agent {} | ownership: 1/{}",
                        group.leader,
                        group.members.len()
                    ));
                    ui.label(format!(
                        "Your shared-property value: {}",
                        wealth / count + u128::from(index < wealth % count)
                    ));
                    ui.label(format!(
                        "Shared cash {} / reserved {}",
                        group.account.wallet(),
                        group.account.reserved_cash()
                    ));
                    for resource in Resource::ALL {
                        ui.label(format!(
                            "Shared {resource}: {}",
                            group.account.inventory().get(resource)
                        ));
                    }
                    ui.label(format!(
                        "Members: {:?}",
                        group.members.iter().map(|id| id.0).collect::<Vec<_>>()
                    ));
                },
            );
        }
        let owns = |party| {
            party == Party::Agent(agent.id)
                || group.is_some_and(|g| party == Party::Organization(g.id))
        };
        let contracts: Vec<_> = view
            .snapshot
            .institutions
            .contracts
            .iter()
            .filter(|c| owns(c.proposer) || owns(c.recipient))
            .collect();
        ui.collapsing(format!("Recurring contracts ({})", contracts.len()), |ui| {
            for contract in contracts {
                ui.label(format!(
                    "#{} {} -> {}",
                    contract.id,
                    party_label(contract.seller()),
                    party_label(contract.buyer())
                ));
                ui.label(format!(
                    "{} {} @ {}, {} shipments left, interval {}",
                    contract.quantity,
                    contract.resource,
                    contract.unit_price,
                    contract.remaining,
                    contract.interval_ticks
                ));
                ui.label(match contract.next_delivery {
                    Some(tick) => format!("Next delivery t{tick}; seller bond {}", contract.bond),
                    None => format!("Pending acceptance until t{}", contract.expires_at),
                });
            }
        });
        let proposals: Vec<_> = view
            .snapshot
            .proposals
            .proposals
            .iter()
            .filter(|p| p.proposer == agent.id || p.recipient == agent.id)
            .collect();
        ui.collapsing(format!("Open proposals ({})", proposals.len()), |ui| {
            for proposal in &proposals {
                let other = if proposal.proposer == agent.id {
                    proposal.recipient
                } else {
                    proposal.proposer
                };
                let direction = if proposal.recipient == agent.id {
                    "Received"
                } else {
                    "Sent"
                };
                ui.label(format!(
                    "{direction} #{} with Agent {other}; expires t{}",
                    proposal.id, proposal.expires_at
                ));
                ui.label(proposal_terms(&proposal.terms));
            }
            if proposals.is_empty() {
                ui.label("No active proposals.");
            }
        });
        ui.collapsing("Recent decisions", |ui| {
            for record in agent.history.iter().rev() {
                ui.label(format!(
                    "#{} {:?}: {:?}",
                    record.tick, record.decision.reason, record.decision.action
                ));
                if let Some(reason) = &record.rejection {
                    ui.colored_label(Color32::LIGHT_RED, reason);
                }
            }
        });
        ui.collapsing(format!("Memories ({})", agent.memories.len()), |ui| {
            egui::ComboBox::from_id_salt("memory_filter")
                .selected_text(self.memory_filter.map_or_else(
                    || "Relevant experiences".to_owned(),
                    |kind| format!("{kind:?}"),
                ))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.memory_filter, None, "Relevant experiences");
                    for kind in [
                        MemoryKind::Episodic,
                        MemoryKind::Economic,
                        MemoryKind::Social,
                        MemoryKind::Goal,
                    ] {
                        ui.selectable_value(
                            &mut self.memory_filter,
                            Some(kind),
                            format!("{kind:?}"),
                        );
                    }
                });
            let mut query = MemoryQuery::new(view.snapshot.world.tick);
            query.kind = self.memory_filter;
            if query.kind.is_none() {
                query.resource = Some(
                    if agent.needs.hunger > 0 || agent.account.available(Resource::Food) < 5 {
                        Resource::Food
                    } else if agent.occupation == Occupation::Producer {
                        Resource::Wood
                    } else {
                        Resource::Goods
                    },
                );
            }
            let memories = recall(agent, &query);
            if memories.is_empty() {
                ui.small("No retained experiences in this category.");
            }
            for entry in memories {
                let memory = entry.memory;
                ui.small(format!(
                    "t{} | {:?} | importance {}",
                    memory.tick,
                    memory.content.kind(),
                    memory.importance
                ));
                ui.label(memory.content.to_string());
                ui.separator();
            }
        });
        ui.collapsing(
            format!("Relationships ({})", agent.relationships.len()),
            |ui| {
                let mut edges: Vec<_> = agent.relationships.iter().collect();
                edges.sort_by_key(|(id, edge)| (std::cmp::Reverse(edge.trust), **id));
                for (&id, edge) in edges {
                    if ui
                        .link(format!(
                            "Agent {id}: trust {} | familiar {} | interactions {}",
                            edge.trust, edge.familiarity, edge.interaction_count
                        ))
                        .clicked()
                    {
                        self.selected = Some(id);
                    }
                }
            },
        );
    }

    fn map(&mut self, ui: &mut egui::Ui, view: &View) {
        ui.horizontal(|ui| {
            ui.heading("World");
            ui.checkbox(&mut self.show_network, "Social network");
            for occupation in [
                Occupation::Farmer,
                Occupation::Producer,
                Occupation::Merchant,
                Occupation::Consumer,
            ] {
                ui.colored_label(job_color(occupation), format!("{occupation:?}"));
            }
        });
        let size = Vec2::new(ui.available_width(), 310.0);
        let (response, painter) = ui.allocate_painter(size, Sense::click());
        let rect = response.rect.shrink(12.0);
        painter.rect_filled(response.rect, 6.0, Color32::from_rgb(18, 25, 35));
        for i in 0..=10 {
            let x = egui::lerp(rect.x_range(), i as f32 / 10.0);
            let y = egui::lerp(rect.y_range(), i as f32 / 10.0);
            painter.line_segment(
                [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
                Stroke::new(0.5_f32, Color32::from_gray(42)),
            );
            painter.line_segment(
                [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
                Stroke::new(0.5_f32, Color32::from_gray(42)),
            );
        }
        let position = |agent: &Agent| {
            Pos2::new(
                rect.left()
                    + f32::from(agent.position.x) / f32::from(view.snapshot.world.width.max(1))
                        * rect.width(),
                rect.top()
                    + f32::from(agent.position.y) / f32::from(view.snapshot.world.height.max(1))
                        * rect.height(),
            )
        };
        if self.show_network {
            for agent in &view.snapshot.world.agents {
                if view.snapshot.world.agents.len() > 200 && Some(agent.id) != self.selected {
                    continue;
                }
                for (&other, edge) in &agent.relationships {
                    if edge.trust < 6 {
                        continue;
                    }
                    if let Some(target) = view.snapshot.world.agents.iter().find(|a| a.id == other)
                    {
                        painter.line_segment(
                            [position(agent), position(target)],
                            Stroke::new(0.6_f32, Color32::from_rgba_unmultiplied(80, 170, 200, 65)),
                        );
                    }
                }
            }
        }
        let mut nearest = None;
        for agent in &view.snapshot.world.agents {
            let pos = position(agent);
            let selected = Some(agent.id) == self.selected;
            painter.circle_filled(
                pos,
                if selected { 6.0 } else { 3.5 },
                job_color(agent.occupation),
            );
            if selected {
                painter.circle_stroke(pos, 9.0, Stroke::new(1.5_f32, Color32::WHITE));
            }
            if let Some(pointer) = response.hover_pos() {
                let distance = pos.distance_sq(pointer);
                if distance < 144.0 && nearest.is_none_or(|(_, best)| distance < best) {
                    nearest = Some((agent.id, distance));
                }
            }
        }
        if response.clicked()
            && let Some((id, _)) = nearest
        {
            self.selected = Some(id);
        }
        if let Some((id, _)) = nearest {
            response.on_hover_text(format!("Agent {id} — click to inspect"));
        }
        let social = agentsociety_social::analyze(view.snapshot.world.agents.iter());
        ui.label(format!("{} agents | {} social edges | mean trust {:.1} | {} trusted groups (including isolates)",
            view.snapshot.world.agents.len(), social.directed_edges, social.mean_trust, social.communities));
    }

    fn economy(&self, ui: &mut egui::Ui, view: &View) {
        ui.heading("Economy");
        let stats = &view.snapshot.stats;
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!("Trades {}", view.snapshot.cumulative.trade_count));
            ui.label(format!("Turnover {}", view.snapshot.cumulative.trade_value));
            ui.label(format!("Cash {}", stats.total_money));
            ui.label(format!("Mean wealth {:.1}", stats.average_wealth));
            ui.label(format!("Gini {:.4}", stats.gini));
            ui.label(format!("Hungry {}", stats.hungry_agents));
        });
        let flows = &view.snapshot.cumulative;
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "Organizations {} | shared cash {}",
                stats.organizations, stats.organization_cash
            ));
            ui.label(format!("Shared batches {}", flows.group_productions));
            ui.label(format!(
                "Contracts {} | deliveries {} | defaults {}",
                stats.open_contracts, flows.contract_deliveries, flows.contract_defaults
            ));
        });
        ui.collapsing("Scenario and public finances", |ui| {
            let scenario = &view.snapshot.scenario;
            ui.label(format!(
                "Treasury {} | taxes {} | redistributed {}",
                stats.treasury_money, flows.taxes, flows.redistributed
            ));
            ui.label(format!(
                "Sales tax {:.2}% | cash levy {:.2}% | redistribute {:.2}% every {} ticks",
                f64::from(scenario.policy.sales_tax_bps) / 100.0,
                f64::from(scenario.policy.cash_levy_bps) / 100.0,
                f64::from(scenario.policy.redistribution_bps) / 100.0,
                scenario.policy.period
            ));
            for effect in &scenario.supply {
                ui.label(format!(
                    "{} replenishment {:.0}% until t{}",
                    effect.resource,
                    f64::from(effect.regeneration_bps) / 100.0,
                    effect.ends_at
                ));
            }
            for resource in Resource::ALL {
                if flows.destroyed[resource.index()] > 0 {
                    ui.label(format!(
                        "{resource} crisis losses: {}",
                        flows.destroyed[resource.index()]
                    ));
                }
            }
            for scheduled in view
                .snapshot
                .config
                .scenario
                .events
                .iter()
                .skip(scenario.next_event)
                .take(8)
            {
                ui.label(format!(
                    "Scheduled t{}: {}",
                    scheduled.tick, scheduled.label
                ));
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("Negotiated trades {}", flows.negotiated_trades));
            ui.label(format!("Joint batches {}", flows.cooperations));
            ui.label(format!("Goal changes {}", flows.goal_changes));
            ui.label(format!(
                "Open proposals {}",
                view.snapshot.proposals.proposals.len()
            ));
        });
        egui::Grid::new("resources")
            .striped(true)
            .num_columns(6)
            .show(ui, |ui| {
                for h in [
                    "Resource",
                    "Price",
                    "Owned",
                    "Produced",
                    "Household",
                    "Factory input",
                ] {
                    ui.strong(h);
                }
                ui.end_row();
                for resource in Resource::ALL {
                    let i = resource.index();
                    ui.label(resource.to_string());
                    ui.label(stats.prices[i].to_string());
                    ui.label(stats.inventories[i].to_string());
                    ui.label(view.snapshot.cumulative.produced[i].to_string());
                    ui.label(view.snapshot.cumulative.household_consumed[i].to_string());
                    ui.label(view.snapshot.cumulative.production_inputs[i].to_string());
                    ui.end_row();
                }
            });
        Plot::new(("prices", view.generation))
            .height(160.0)
            .legend(egui_plot::Legend::default())
            .x_axis_label("Tick")
            .y_axis_label("Price (minor units)")
            .show(ui, |plot| {
                for resource in Resource::ALL {
                    let points: PlotPoints<'_> = view
                        .history
                        .iter()
                        .map(|s| [s.tick as f64, s.prices[resource.index()] as f64])
                        .collect();
                    plot.line(Line::new(resource.to_string(), points));
                }
            });
        Plot::new(("gini", view.generation))
            .height(100.0)
            .include_y(0.0)
            .include_y(1.0)
            .y_axis_label("Gini")
            .show(ui, |plot| {
                plot.line(Line::new(
                    "Gini",
                    PlotPoints::from_iter(view.history.iter().map(|s| [s.tick as f64, s.gini])),
                ));
            });
    }

    fn timeline(&mut self, ui: &mut egui::Ui, view: &View) {
        ui.horizontal(|ui| {
            ui.heading("Event timeline");
            ui.checkbox(&mut self.selected_events_only, "Selected agent only");
            egui::ComboBox::from_id_salt("event_filter")
                .selected_text(
                    [
                        "All",
                        "Trades",
                        "Production",
                        "Social",
                        "Decisions",
                        "Proposals",
                        "Cooperation",
                        "Goals",
                        "Organizations",
                        "Contracts",
                        "Scenario",
                    ][self.event_filter],
                )
                .show_ui(ui, |ui| {
                    for (i, name) in [
                        "All",
                        "Trades",
                        "Production",
                        "Social",
                        "Decisions",
                        "Proposals",
                        "Cooperation",
                        "Goals",
                        "Organizations",
                        "Contracts",
                        "Scenario",
                    ]
                    .iter()
                    .enumerate()
                    {
                        ui.selectable_value(&mut self.event_filter, i, *name);
                    }
                });
            ui.small(format!("Last {} events", view.events.len()));
        });
        egui::ScrollArea::vertical()
            .max_height(165.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for record in &view.events {
                    let category = match &record.event {
                        Event::Organization { .. } => 8,
                        Event::Contract { .. } => 9,
                        Event::Scenario { .. } => 10,
                        Event::TradeExecuted { .. } | Event::NegotiatedTrade { .. } => 1,
                        Event::ProposalOpened { .. }
                        | Event::ProposalCountered { .. }
                        | Event::ProposalClosed { .. } => 5,
                        Event::CooperationCompleted { .. } => 6,
                        Event::GoalChanged { .. } => 7,
                        Event::ResourceProduced { .. } | Event::ResourceConsumed { .. } => 2,
                        Event::RelationshipUpdated { .. } | Event::RelationshipForgotten { .. } => {
                            3
                        }
                        Event::ExternalDecisionRecorded { .. }
                        | Event::DecisionRecorded { .. }
                        | Event::ActionRejected { .. } => 4,
                        _ => 0,
                    };
                    if self.event_filter != 0 && self.event_filter != category {
                        continue;
                    }
                    if self.selected_events_only
                        && self.selected.is_none_or(|id| !involves(&record.event, id))
                    {
                        continue;
                    }
                    ui.monospace(format!(
                        "#{} t{}  {}",
                        record.sequence,
                        record.tick,
                        describe(&record.event)
                    ));
                }
            });
    }
}

impl Dashboard {
    fn render(&mut self, context: &egui::Context) {
        let view = self.worker.view();
        if self.generation != view.generation {
            self.generation = view.generation;
            self.population = view.snapshot.config.agents;
            self.seed = view.snapshot.config.seed;
            self.interactions = view.snapshot.config.interactions.clone();
            self.organizations = view.snapshot.config.organizations.clone();
            self.scenario = view.snapshot.config.scenario.clone();
            self.selected = (self.population > 0).then_some(AgentId(0));
            self.error = None;
        }
        egui::TopBottomPanel::top("controls").show(context, |ui| self.controls(ui, &view));
        egui::SidePanel::right("inspector")
            .default_width(300.0)
            .min_width(230.0)
            .show(context, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.inspector(ui, &view));
            });
        egui::TopBottomPanel::bottom("timeline")
            .resizable(true)
            .default_height(205.0)
            .show(context, |ui| self.timeline(ui, &view));
        egui::CentralPanel::default().show(context, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.map(ui, &view);
                ui.separator();
                self.economy(ui, &view);
            });
        });
        if let Some(selection) = self.picker.as_mut().and_then(|picker| picker.show(context)) {
            if let Some(parent) = selection.path.parent() {
                self.file_directory = parent.to_path_buf();
            }
            match selection.purpose {
                Purpose::Save => self.command(Command::Save {
                    path: selection.path,
                    overwrite: selection.overwrite,
                }),
                Purpose::Load => self.command(Command::Load(selection.path)),
                Purpose::Branch => self.command(Command::Branch(selection.path)),
                Purpose::Scenario => match agentsociety_storage::read_scenario(&selection.path) {
                    Ok(scenario) => {
                        self.scenario = scenario;
                        self.error = None;
                    }
                    Err(error) => self.error = Some(format!("Scenario failed: {error:#}")),
                },
            }
        }
        if self.picker.as_ref().is_some_and(|p| !p.open) {
            self.picker = None;
        }
        if self
            .exit_after
            .is_some_and(|tick| view.snapshot.world.tick >= tick)
        {
            context.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        context.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

impl eframe::App for Dashboard {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.render(context);
    }
}

fn party_label(party: Party) -> String {
    match party {
        Party::Agent(id) => format!("Agent {id}"),
        Party::Organization(id) => format!("Group {}", id.0),
    }
}

fn job_color(occupation: Occupation) -> Color32 {
    match occupation {
        Occupation::Farmer => Color32::from_rgb(108, 213, 137),
        Occupation::Producer => Color32::from_rgb(110, 168, 255),
        Occupation::Merchant => Color32::from_rgb(250, 193, 83),
        Occupation::Consumer => Color32::from_rgb(217, 146, 238),
    }
}

fn involves(event: &Event, id: AgentId) -> bool {
    event.participants().contains(&id)
}

fn describe(event: &Event) -> String {
    match event {
        Event::Organization { event } => match event {
            OrganizationEvent::Founded {
                organization,
                founder,
                stake,
            } => format!(
                "Group {} founded by Agent {founder}; stake {stake}",
                organization.0
            ),
            OrganizationEvent::Joined {
                organization,
                agent_id,
                ..
            } => format!("Agent {agent_id} joined Group {}", organization.0),
            OrganizationEvent::Contributed {
                organization,
                agent_id,
                cash,
                inventory,
            } => format!(
                "Agent {agent_id} contributed {cash} cash, {:?} stock to Group {}",
                inventory.amounts(),
                organization.0
            ),
            OrganizationEvent::Left {
                organization,
                agent_id,
                cash,
                inventory,
                ..
            } => format!(
                "Agent {agent_id} left Group {}; received {cash} cash, {:?} stock",
                organization.0,
                inventory.amounts()
            ),
            OrganizationEvent::Produced {
                organization,
                goods,
                ..
            } => format!("Group {} produced {goods} shared goods", organization.0),
            OrganizationEvent::Distributed {
                organization,
                cash_per_member,
                goods_per_member,
                ..
            } => format!(
                "Group {} dividend: {cash_per_member} cash and {goods_per_member} goods per member",
                organization.0
            ),
        },
        Event::Contract { event } => match event {
            ContractEvent::Offered { contract, .. } => format!(
                "Contract #{} offered: {} -> {}, {} shipments of {} {} @ {}",
                contract.id,
                party_label(contract.seller()),
                party_label(contract.buyer()),
                contract.deliveries,
                contract.quantity,
                contract.resource,
                contract.unit_price
            ),
            ContractEvent::Accepted { contract, .. } => format!(
                "Contract #{} funded; first delivery t{}",
                contract.id,
                contract.next_delivery.unwrap_or(0)
            ),
            ContractEvent::Delivered {
                contract_id,
                quantity,
                resource,
                price,
                ..
            } => format!("Contract #{contract_id} delivered {quantity} {resource} @ {price}"),
            ContractEvent::Closed {
                contract,
                outcome,
                compensation,
                ..
            } => format!(
                "Contract #{} {outcome:?}; compensation {compensation}",
                contract.id
            ),
        },
        Event::Scenario { event } => match event {
            ScenarioEvent::Applied { scheduled, .. } => format!("Scenario: {}", scheduled.label),
            ScenarioEvent::SupplyRestored { resource } => {
                format!("{resource} replenishment restored")
            }
            ScenarioEvent::InventoryLost {
                owner,
                resource,
                quantity,
                ..
            } => format!(
                "Crisis loss: {} lost {quantity} {resource}",
                party_label(*owner)
            ),
            ScenarioEvent::TaxCollected {
                payer, sales, levy, ..
            } => format!(
                "{} paid sales tax {sales}, cash levy {levy}",
                party_label(*payer)
            ),
            ScenarioEvent::Redistribution { agent_id, amount } => {
                format!("Treasury paid {amount} to Agent {agent_id}")
            }
        },
        Event::ExternalDecisionRecorded { result } => format!(
            "AI Agent {}: {:?} — {}",
            result.agent_id, result.outcome, result.summary
        ),
        Event::ProposalOpened { proposal } => format!(
            "Offer #{}: {} -> {}, {} (until t{})",
            proposal.id,
            proposal.proposer,
            proposal.recipient,
            proposal_terms(&proposal.terms),
            proposal.expires_at
        ),
        Event::ProposalCountered { proposal } => format!(
            "Counter #{}: {} -> {}, {}",
            proposal.id,
            proposal.proposer,
            proposal.recipient,
            proposal_terms(&proposal.terms)
        ),
        Event::ProposalClosed { proposal, outcome } => format!(
            "Proposal #{} {:?}: {} / {}",
            proposal.id, outcome, proposal.proposer, proposal.recipient
        ),
        Event::CooperationCompleted {
            producer,
            partner,
            producer_share,
            partner_share,
            ..
        } => format!(
            "Joint batch: producer {producer} received {producer_share}, partner {partner} received {partner_share} goods"
        ),
        Event::GoalChanged {
            agent_id,
            previous,
            goal,
            reason,
            ..
        } => format!("Goal Agent {agent_id}: {previous:?} -> {goal:?} ({reason:?})"),
        Event::NegotiatedTrade {
            buyer,
            seller,
            resource,
            quantity,
            price,
            ..
        } => format!("Negotiated trade: {seller} -> {buyer}, {quantity} {resource} @ {price}"),
        Event::TradeExecuted {
            buyer,
            seller,
            resource,
            quantity,
            price,
            ..
        } => format!("Trade: {seller} -> {buyer}, {quantity} {resource} @ {price}"),
        Event::RelationshipUpdated {
            agent_id,
            other,
            relationship,
        } => format!(
            "Social: {agent_id} -> {other}, trust {}",
            relationship.trust
        ),
        Event::DecisionRecorded {
            agent_id, decision, ..
        } => format!(
            "Agent {agent_id}: {:?} ({:?})",
            decision.action, decision.reason
        ),
        _ => format!("{event:?}"),
    }
}

fn proposal_terms(terms: &ProposalTerms) -> String {
    match terms {
        ProposalTerms::Trade {
            side,
            resource,
            quantity,
            unit_price,
        } => format!("{side:?} {quantity} {resource} @ {unit_price}"),
        ProposalTerms::Cooperation { producer_share } => format!(
            "Joint production: producer {producer_share} / partner {} goods",
            COOPERATIVE_GOODS - producer_share
        ),
    }
}

#[cfg(test)]
#[path = "dashboard_tests.rs"]
mod input_tests;
