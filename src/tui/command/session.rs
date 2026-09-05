use minuet::kernel::{KernelError, KernelHandle};

pub async fn new(kernel: &KernelHandle) -> Result<String, KernelError> {
    Ok(format!(
        "started memory session {}",
        kernel.new_session().await?
    ))
}

pub async fn clear(kernel: &KernelHandle) -> Result<String, KernelError> {
    kernel.clear_session().await?;
    Ok("cleared conversation history".into())
}
