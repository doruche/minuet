use std::sync::Arc;

use super::{
    Tool, current_datetime::CurrentDatetimeTool, echo::EchoTool, random_integer::RandomIntegerTool,
};

pub(super) fn all() -> [Arc<dyn Tool>; 3] {
    [
        Arc::new(EchoTool) as Arc<dyn Tool>,
        Arc::new(RandomIntegerTool),
        Arc::new(CurrentDatetimeTool),
    ]
}
