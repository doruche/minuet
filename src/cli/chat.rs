use std::{
    error::Error,
    io::{self, Write},
    process::ExitCode,
};

use minuet::{agent_loop::RunStopReason, kernel::KernelHandle};

pub async fn run(kernel: KernelHandle, prompt: String) -> Result<ExitCode, Box<dyn Error>> {
    let outcome = tokio::select! {
        biased;
        signal = tokio::signal::ctrl_c() => {
            signal?;
            // Dropping the reply does not cancel accepted execution. main owns
            // the shutdown barrier and joins the kernel before process exit.
            writeln!(io::stderr(), "minuet: interrupted; waiting for shutdown")?;
            return Ok(ExitCode::from(130));
        },
        outcome = kernel.run(prompt) => outcome?,
    };

    // stdout is the model's final text, not the TUI transcript. Preserve tabs
    // and controls as data and add only a missing final newline for shell use.
    let mut stdout = io::stdout().lock();
    stdout.write_all(outcome.text.as_bytes())?;
    if !outcome.text.is_empty() && !outcome.text.ends_with('\n') {
        stdout.write_all(b"\n")?;
    }
    stdout.flush()?;

    match outcome.stop_reason {
        RunStopReason::Completed => Ok(ExitCode::SUCCESS),
        RunStopReason::StepLimit => {
            writeln!(
                io::stderr(),
                "minuet: model-turn limit reached; pending tool calls were not executed"
            )?;
            Ok(ExitCode::FAILURE)
        },
    }
}
