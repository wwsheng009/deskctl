use std::sync::Arc;
use std::time::Duration;

use deskctl_protocol::{ErrorCode, ToolError};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceKind {
    Vision,
    Capture,
    Semantic,
    Input,
}

pub struct ResourcePermits {
    _permits: Vec<OwnedSemaphorePermit>,
}

pub struct ResourceGate {
    vision: Arc<Semaphore>,
    capture: Arc<Semaphore>,
    semantic: Arc<Semaphore>,
    input: Arc<Semaphore>,
    wait: Duration,
}

impl ResourceGate {
    pub fn new(wait: Duration) -> Self {
        Self {
            vision: Arc::new(Semaphore::new(1)),
            capture: Arc::new(Semaphore::new(4)),
            semantic: Arc::new(Semaphore::new(1)),
            input: Arc::new(Semaphore::new(1)),
            wait,
        }
    }

    pub async fn acquire(&self, kinds: &[ResourceKind]) -> Result<ResourcePermits, ToolError> {
        let mut unique = kinds.to_vec();
        unique.sort();
        unique.dedup();
        let mut permits = Vec::with_capacity(unique.len());
        for kind in unique {
            let semaphore = match kind {
                ResourceKind::Vision => self.vision.clone(),
                ResourceKind::Capture => self.capture.clone(),
                ResourceKind::Semantic => self.semantic.clone(),
                ResourceKind::Input => self.input.clone(),
            };
            let permit = tokio::time::timeout(self.wait, semaphore.acquire_owned())
                .await
                .map_err(|_| {
                    ToolError::new(ErrorCode::ResourceBusy, "desktop resource queue timed out")
                        .retryable(true)
                })?
                .map_err(|_| {
                    ToolError::new(ErrorCode::ResourceBusy, "desktop resource gate closed")
                        .retryable(true)
                })?;
            permits.push(permit);
        }
        Ok(ResourcePermits { _permits: permits })
    }
}
