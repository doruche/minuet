use async_trait::async_trait;

use super::{AgentLoop, LoopContext, LoopError, RunOutcome, RunStopReason, UsageSummary};

pub struct ReactLoop {
    max_steps: usize,
}

impl ReactLoop {
    pub fn new(max_steps: usize) -> Result<Self, LoopError> {
        if max_steps == 0 {
            return Err(LoopError::InvalidMaxSteps);
        }
        Ok(Self { max_steps })
    }
}

#[async_trait]
impl AgentLoop for ReactLoop {
    async fn run(&self, context: &mut LoopContext<'_>) -> Result<RunOutcome, LoopError> {
        let mut activities = Vec::new();
        let mut run_usage = UsageSummary::default();

        for model_turn in 1..=self.max_steps {
            let turn = context.infer_and_commit().await?;
            run_usage.observe(turn.usage);

            if !context.has_pending_tools() {
                return Ok(RunOutcome {
                    text: turn.text_summary(),
                    model_turns: model_turn,
                    tool_activity: activities,
                    usage: run_usage,
                    stop_reason: RunStopReason::Completed,
                });
            }
            if model_turn == self.max_steps {
                let skipped = context.skip_pending().await?;
                activities.extend(skipped.activities);
                return Ok(RunOutcome {
                    text: turn.text_summary(),
                    model_turns: model_turn,
                    tool_activity: activities,
                    usage: run_usage,
                    stop_reason: RunStopReason::StepLimit,
                });
            }

            let tool_round = context.invoke_pending().await?;
            activities.extend(tool_round.activities);
        }

        unreachable!("max_steps is non-zero and every loop path returns or continues")
    }
}
