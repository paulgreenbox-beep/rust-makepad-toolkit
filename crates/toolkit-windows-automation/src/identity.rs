use rudra_runtime::{IdAllocator, IdExhausted};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

struct WindowToken;
struct ElementToken;

pub(crate) struct SessionIds {
    nonce: String,
    windows: IdAllocator<WindowToken>,
    elements: IdAllocator<ElementToken>,
}

impl SessionIds {
    pub(crate) fn new() -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self::with_nonce(format!("{:x}{:x}", process::id(), timestamp))
    }

    pub(crate) fn with_nonce(nonce: String) -> Self {
        Self {
            nonce,
            windows: IdAllocator::new(),
            elements: IdAllocator::new(),
        }
    }

    pub(crate) fn window(&mut self) -> Result<String, IdExhausted> {
        self.windows
            .allocate()
            .map(|id| format!("win:{}:{}", self.nonce, id.get()))
    }

    pub(crate) fn element(&mut self) -> Result<String, IdExhausted> {
        self.elements
            .allocate()
            .map(|id| format!("el:{}:{}", self.nonce, id.get()))
    }

    pub(crate) fn owns_window(&self, id: &str) -> bool {
        id.starts_with(&format!("win:{}:", self.nonce))
    }

    pub(crate) fn owns_element(&self, id: &str) -> bool {
        id.starts_with(&format!("el:{}:", self.nonce))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_opaque_typed_and_monotonic_within_session() {
        let mut ids = SessionIds::with_nonce("session".into());
        assert_eq!(ids.window().unwrap(), "win:session:1");
        assert_eq!(ids.window().unwrap(), "win:session:2");
        assert_eq!(ids.element().unwrap(), "el:session:1");
        assert!(ids.owns_window("win:session:1"));
        assert!(!ids.owns_window("win:other:1"));
        assert!(!ids.owns_element("win:session:1"));
    }
}
