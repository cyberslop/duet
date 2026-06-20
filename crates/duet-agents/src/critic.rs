//! Critic backends: a CLI-agent wrapper (reads files via tools) and the local
//! HTTP backend (the unit is inlined into its prompt). Both implement
//! `duet_core::agent::Critic`, so the engine talks only to the trait.

use crate::cli::agent_for;
use crate::local::{default_endpoint, LocalBackend};
use anyhow::{anyhow, Result};
use duet_core::agent::{run_stream, Agent, Critic, Ctx, ReviewReq, Role};
use duet_core::events::{claude_final_result, extract_json};
use duet_core::render::Model;
use duet_core::report::{Reporter, Sys};

/// Wraps a CLI agent (Claude/Codex) as a critic.
pub struct CliCritic {
    agent: Box<dyn Agent>,
}

impl CliCritic {
    pub fn new(agent: Box<dyn Agent>) -> Self {
        Self { agent }
    }
}

impl Critic for CliCritic {
    fn model(&self) -> Model {
        self.agent.model()
    }
    fn can_build(&self) -> bool {
        true
    }
    fn review(&self, req: &ReviewReq, ctx: &Ctx, rep: &dyn Reporter) -> Result<()> {
        let model = self.agent.model();
        let out_arg = matches!(model, Model::Codex).then_some(req.out);
        run_stream(&*self.agent, req.role, req.cli_prompt, out_arg, req.raw, rep, ctx)?;
        // Claude has no -o flag; recover its final message from the captured stream.
        if matches!(model, Model::Claude) {
            let raw = std::fs::read_to_string(req.raw).unwrap_or_default();
            let mut result = claude_final_result(&raw).unwrap_or_default();
            if req.strip {
                result = extract_json(&result);
            }
            std::fs::write(req.out, result)?;
        }
        Ok(())
    }
    fn swapped(self: Box<Self>, builder: Box<dyn Agent>) -> (Box<dyn Agent>, Box<dyn Critic>) {
        (self.agent, Box::new(CliCritic::new(builder)))
    }
}

/// The local OpenAI-compatible backend as a critic (chat-only → critic-only).
pub struct LocalCritic {
    backend: LocalBackend,
}

impl Critic for LocalCritic {
    fn model(&self) -> Model {
        Model::Local
    }
    fn can_build(&self) -> bool {
        false
    }
    fn review(&self, req: &ReviewReq, _ctx: &Ctx, rep: &dyn Reporter) -> Result<()> {
        let schema = matches!(req.role, Role::ReviewJson).then_some(req.schema);
        let content = self.backend.critique(req.local_system, req.local_user, schema, req.raw, rep)?;
        let content = if req.strip { extract_json(&content) } else { content };
        std::fs::write(req.out, content)?;
        Ok(())
    }
    fn swapped(self: Box<Self>, builder: Box<dyn Agent>) -> (Box<dyn Agent>, Box<dyn Critic>) {
        // Never reached: a local critic reports can_build() == false, and the
        // engine guards swap on that. Return roles unchanged for totality.
        (builder, self)
    }
}

/// Construct the critic backend for a model id.
pub fn build_critic(
    model: Model,
    endpoint: Option<&str>,
    model_name: Option<&str>,
    rep: &dyn Reporter,
) -> Result<Box<dyn Critic>> {
    if model == Model::Local {
        let endpoint = endpoint.map(String::from).unwrap_or_else(default_endpoint);
        let name = model_name
            .map(String::from)
            .or_else(|| std::env::var("DUET_LOCAL_MODEL").ok())
            .or_else(|| LocalBackend::list_models(&endpoint, 10).ok().and_then(|m| m.into_iter().next()))
            .ok_or_else(|| anyhow!("no local model available at {endpoint} — start LM Studio and load a model"))?;
        rep.sys(Sys::Note, &format!("local critic: {name} @ {endpoint}"));
        Ok(Box::new(LocalCritic { backend: LocalBackend::new(&endpoint, &name, std::env::var("LOCAL_API_KEY").ok(), 600) }))
    } else {
        Ok(Box::new(CliCritic::new(agent_for(model))))
    }
}
