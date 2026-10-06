//! Public API boundary used by AI clients, scripts, and SDK consumers.
use serde_json::Value;

pub type CanonicalOperationResult = Value;

pub trait Transport {
    fn call(&mut self, operation: &str, arguments: Value) -> CanonicalOperationResult;
}

pub struct InProcessTransport<F> {
    dispatcher: F,
}

impl<F> InProcessTransport<F> {
    pub fn new(dispatcher: F) -> Self {
        Self { dispatcher }
    }
}

impl<F> Transport for InProcessTransport<F>
where
    F: FnMut(&str, Value) -> CanonicalOperationResult,
{
    fn call(&mut self, operation: &str, arguments: Value) -> CanonicalOperationResult {
        (self.dispatcher)(operation, arguments)
    }
}

pub struct AiUiClient<T> {
    transport: T,
}

impl<T: Transport> AiUiClient<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub fn call(&mut self, operation: &str, arguments: Value) -> CanonicalOperationResult {
        self.transport.call(operation, arguments)
    }

    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn client_uses_only_dispatcher_boundary() {
        let transport = InProcessTransport::new(
            |operation: &str, arguments: Value| json!({"success":true,"operation":operation,"arguments":arguments}),
        );
        let mut client = AiUiClient::new(transport);
        let result = client.call("inspect_ui", json!({"session_id":"s1"}));
        assert_eq!(result["operation"], "inspect_ui");
        assert_eq!(result["arguments"]["session_id"], "s1");
    }
}
