//! Small Rust SDK facade. Every method is a public operation call.
use crate::api::{AiUiClient, CanonicalOperationResult, Transport};
use serde_json::{Value, json};

pub struct AiUiSdk<T: Transport> {
    client: AiUiClient<T>,
}
impl<T: Transport> AiUiSdk<T> {
    pub fn connect(transport: T) -> Self {
        Self {
            client: AiUiClient::new(transport),
        }
    }
    pub fn call(&mut self, operation: &str, arguments: Value) -> CanonicalOperationResult {
        self.client.call(operation, arguments)
    }
    pub fn session_open(&mut self, application: Value) -> CanonicalOperationResult {
        self.call("session.open", json!({"application":application}))
    }
    pub fn set_value(
        &mut self,
        session_id: &str,
        id: &str,
        value: Value,
    ) -> CanonicalOperationResult {
        self.call(
            "set_value",
            json!({"session_id":session_id,"id":id,"value":value}),
        )
    }
    pub fn click(&mut self, session_id: &str, id: &str) -> CanonicalOperationResult {
        self.call("click", json!({"session_id":session_id,"id":id}))
    }
    pub fn assert_visible(&mut self, session_id: &str, id: &str) -> CanonicalOperationResult {
        self.call("assert_visible", json!({"session_id":session_id,"id":id}))
    }
}
