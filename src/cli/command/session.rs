use minuet::kernel::{KernelError, KernelHandle};

pub async fn new(kernel: &KernelHandle) -> Result<String, KernelError> {
    Ok(format!(
        "started memory session {}",
        kernel.new_session().await?
    ))
}
