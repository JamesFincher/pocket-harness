use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeatureDefinition {
    pub key: &'static str,
    pub description: &'static str,
    pub connector_capability: Option<&'static str>,
}

pub fn registry() -> &'static [FeatureDefinition] {
    &[
        FeatureDefinition {
            key: "gateway.telegram",
            description: "Telegram polling and message delivery.",
            connector_capability: None,
        },
        FeatureDefinition {
            key: "gateway.llm_router",
            description: "Parent-level natural language command routing and reply polish.",
            connector_capability: None,
        },
        FeatureDefinition {
            key: "jobs.queue",
            description: "Durable per-thread request queues.",
            connector_capability: None,
        },
        FeatureDefinition {
            key: "jobs.cancel",
            description: "Cancel a running connector job when supported.",
            connector_capability: Some("connector.cancel"),
        },
        FeatureDefinition {
            key: "jobs.history",
            description: "Safe job receipts without raw prompt/response transcripts.",
            connector_capability: None,
        },
        FeatureDefinition {
            key: "threads.named",
            description: "Named mobile threads with connector, cwd, and feature preferences.",
            connector_capability: None,
        },
        FeatureDefinition {
            key: "threads.cwd",
            description: "Per-thread working directory routing.",
            connector_capability: Some("threads.cwd"),
        },
        FeatureDefinition {
            key: "threads.watch",
            description: "Live progress/status updates for a thread.",
            connector_capability: Some("connector.stream"),
        },
        FeatureDefinition {
            key: "attachments.images",
            description: "Download mobile images and pass private file paths to connectors.",
            connector_capability: Some("attachments.images"),
        },
        FeatureDefinition {
            key: "attachments.files",
            description: "Send local files back to the mobile gateway.",
            connector_capability: None,
        },
        FeatureDefinition {
            key: "mac.screenshot",
            description: "Capture the current Mac display when the gateway has permission.",
            connector_capability: None,
        },
        FeatureDefinition {
            key: "mac.terminal",
            description: "Parent-owned persistent PTY sessions.",
            connector_capability: None,
        },
        FeatureDefinition {
            key: "connector.health",
            description: "Connector health checks before config promotion.",
            connector_capability: Some("connector.health"),
        },
        FeatureDefinition {
            key: "connector.run",
            description: "Run a user request through the selected connector.",
            connector_capability: Some("connector.run"),
        },
        FeatureDefinition {
            key: "connector.status",
            description: "Ask a connector for runtime status when supported.",
            connector_capability: Some("connector.status"),
        },
        FeatureDefinition {
            key: "connector.capabilities",
            description: "Ask a connector to report its supported feature set.",
            connector_capability: Some("connector.capabilities"),
        },
    ]
}
