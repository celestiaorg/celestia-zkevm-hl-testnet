use crate::error::{IsmClientError, Result};
use crate::proto::celestia::zkism::v1::{
    query_client::QueryClient, QueryIsmRequest, QueryIsmResponse, QueryIsmsRequest, QueryIsmsResponse,
};
use crate::types::{ClientConfig, TxResponse};

use anyhow::Context;
use celestia_grpc::{GrpcClient, IntoProtobufAny};
use prost::Message;
use tonic::{
    transport::{Channel, Endpoint},
    Request,
};
use tracing::{debug, info, warn};

/// Celestia gRPC client for proof submission
#[derive(Clone)]
pub struct CelestiaIsmClient {
    pub config: ClientConfig,
    channel: Channel,
    tx_client: GrpcClient,
}

impl CelestiaIsmClient {
    /// Create a new Celestia proof client
    pub async fn new(config: ClientConfig) -> Result<Self> {
        debug!("Creating Celestia proof client with endpoint: {}", config.grpc_endpoint);

        // optional: set timeouts, concurrency limits, TLS, etc.
        let endpoint = Endpoint::from_shared(config.grpc_endpoint.clone())?
            .connect_timeout(std::time::Duration::from_secs(15))
            .tcp_nodelay(true);

        let channel = endpoint.connect().await?;

        let tx_client = GrpcClient::builder()
            .url(&config.grpc_endpoint)
            .private_key_hex(&config.private_key_hex)
            .build()
            .context("Failed to build Lumina gRPC client")?;

        info!("Successfully created Celestia proof client");

        Ok(Self {
            config,
            channel,
            tx_client,
        })
    }

    /// Get the gRPC tx client reference for direct access to Lumina functionality
    pub fn tx_client(&self) -> &GrpcClient {
        &self.tx_client
    }

    /// Get the client configuration
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// Get the configured chain ID
    pub fn chain_id(&self) -> &str {
        &self.config.chain_id
    }

    /// Get the configured ism ID
    pub fn ism_id(&self) -> &str {
        &self.config.ism_id
    }

    /// Get the configured gRPC endpoint
    pub fn grpc_endpoint(&self) -> &str {
        &self.config.grpc_endpoint
    }

    /// Get the cached bech32-encoded signer address
    pub fn signer_address(&self) -> &str {
        &self.config.signer_address
    }

    pub async fn ism(&self, req: QueryIsmRequest) -> Result<QueryIsmResponse> {
        let mut client = QueryClient::new(self.channel.clone());
        let resp = client.ism(Request::new(req)).await?;
        Ok(resp.into_inner())
    }

    pub async fn isms(&self, req: QueryIsmsRequest) -> Result<QueryIsmsResponse> {
        let mut client = QueryClient::new(self.channel.clone());
        let resp = client.isms(Request::new(req)).await?;
        Ok(resp.into_inner())
    }

    /// Query hyperlane ISMs from the interchain_security module
    pub async fn hyperlane_isms(
        &self,
        req: crate::proto::hyperlane::core::interchain_security::v1::QueryIsmsRequest,
    ) -> Result<crate::proto::hyperlane::core::interchain_security::v1::QueryIsmsResponse> {
        use crate::proto::hyperlane::core::interchain_security::v1::query_client::QueryClient as HyperlaneQueryClient;

        let mut client = HyperlaneQueryClient::new(self.channel.clone());
        let resp = client.isms(Request::new(req)).await?;
        Ok(resp.into_inner())
    }

    /// Create a Noop ISM using the tonic-generated client (returns ID directly)
    pub async fn create_noop_ism(
        &self,
        msg: crate::proto::hyperlane::core::interchain_security::v1::MsgCreateNoopIsm,
    ) -> Result<crate::proto::hyperlane::core::interchain_security::v1::MsgCreateNoopIsmResponse> {
        use crate::proto::hyperlane::core::interchain_security::v1::msg_client::MsgClient;

        let mut client = MsgClient::new(self.channel.clone());
        let resp = client
            .create_noop_ism(Request::new(msg))
            .await
            .map_err(|e| IsmClientError::TxFailed(format!("Failed to create Noop ISM: {}", e)))?;
        Ok(resp.into_inner())
    }

    /// Create an Aggregation ISM using the tonic-generated client (returns ID directly)
    pub async fn create_aggregation_ism(
        &self,
        msg: crate::proto::hyperlane::core::interchain_security::v1::MsgCreateAggregationIsm,
    ) -> Result<crate::proto::hyperlane::core::interchain_security::v1::MsgCreateAggregationIsmResponse> {
        use crate::proto::hyperlane::core::interchain_security::v1::msg_client::MsgClient;

        let mut client = MsgClient::new(self.channel.clone());
        let resp = client
            .create_aggregation_ism(Request::new(msg))
            .await
            .map_err(|e| IsmClientError::TxFailed(format!("Failed to create Aggregation ISM: {}", e)))?;
        Ok(resp.into_inner())
    }

    /// Sign and send a tx to Celestia including the provided message.
    pub async fn send_tx<M>(&self, message: M) -> Result<TxResponse>
    where
        M: Message + IntoProtobufAny + Send + Clone + 'static,
    {
        let message_type = message.clone().into_any().type_url;
        debug!(
            "Submitting {} message to Celestia (endpoint: {}, chain: {})",
            message_type, self.config.grpc_endpoint, self.config.chain_id
        );

        let tx_config = celestia_grpc::TxConfig {
            gas_limit: Some(self.config.max_gas),
            gas_price: Some(self.config.gas_price as f64),
            memo: Some("celestia-zkism-client".to_string()),
            ..Default::default()
        };

        match self.tx_client.submit_message(message, tx_config).await {
            Ok(tx_info) => {
                info!(
                    "Successfully submitted {} message: tx_hash={}, height={}",
                    message_type,
                    tx_info.hash,
                    tx_info.height.value()
                );

                Ok(TxResponse {
                    tx_hash: tx_info.hash.to_string(),
                    height: tx_info.height.value(),
                    gas_used: 0, // TxInfo doesn't provide gas_used, use estimation
                    success: true,
                    error_message: None,
                    raw_data: None,
                })
            }
            Err(e) => {
                warn!("Failed to submit {} message: {}", message_type, e);
                Err(IsmClientError::TxFailed(format!(
                    "Failed to submit {message_type}: {e}"
                )))
            }
        }
    }

    /// Wait for a transaction to be confirmed by polling for the new ISM
    async fn wait_for_ism_creation(
        &self,
        ids_before: &std::collections::HashSet<String>,
        max_retries: u32,
    ) -> Result<String> {
        use crate::proto::celestia::zkism::v1::QueryIsmsRequest;

        for attempt in 0..max_retries {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            match self.isms(QueryIsmsRequest { pagination: None }).await {
                Ok(isms_after) => {
                    for ism in isms_after.isms {
                        if !ids_before.contains(&ism.id) {
                            info!("Found newly created ISM with ID: {} (attempt {})", ism.id, attempt + 1);
                            return Ok(ism.id);
                        }
                    }
                    debug!("ISM not found yet, attempt {}/{}", attempt + 1, max_retries);
                }
                Err(e) => {
                    warn!("Failed to query ISMs on attempt {}: {}", attempt + 1, e);
                }
            }
        }

        Err(IsmClientError::TxFailed(format!(
            "ISM was not found after {} attempts",
            max_retries
        )))
    }

    /// Create an ISM and return the created ISM ID.
    /// Uses the same pattern as zkism: query before, submit tx, query after to find new ISM.
    pub async fn create_ism_with_response<M>(&self, message: M) -> Result<crate::types::IsmCreationResponse>
    where
        M: Message + IntoProtobufAny + Send + Clone + 'static,
    {
        let message_type_url = message.clone().into_any().type_url;

        // For zkism module messages
        if message_type_url.contains("celestia.zkism") {
            use crate::proto::celestia::zkism::v1::QueryIsmsRequest;

            let isms_before = self.isms(QueryIsmsRequest { pagination: None }).await?;
            let ids_before: std::collections::HashSet<String> =
                isms_before.isms.iter().map(|ism| ism.id.clone()).collect();

            let tx_response = self.send_tx(message).await?;
            let ism_id = self.wait_for_ism_creation(&ids_before, 20).await?;

            Ok(crate::types::IsmCreationResponse { tx_response, ism_id })
        } else {
            // For hyperlane ISM messages - same pattern
            use crate::proto::hyperlane::core::interchain_security::v1::QueryIsmsRequest as HyperlaneQueryIsmsRequest;

            let isms_before = self
                .hyperlane_isms(HyperlaneQueryIsmsRequest { pagination: None })
                .await?;
            let ids_before: std::collections::HashSet<String> = isms_before
                .isms
                .iter()
                .filter_map(|ism_any| self.decode_hyperlane_ism_id(ism_any))
                .collect();

            let tx_response = self.send_tx(message).await?;
            let ism_id = self.wait_for_hyperlane_ism_creation(&ids_before, 20).await?;

            Ok(crate::types::IsmCreationResponse { tx_response, ism_id })
        }
    }

    /// Helper method to decode ISM ID from a protobuf Any message
    fn decode_hyperlane_ism_id(&self, ism_any: &prost_types::Any) -> Option<String> {
        use prost::Message;
        let type_url = &ism_any.type_url;

        if type_url.contains("NoopISM") {
            use crate::proto::hyperlane::core::interchain_security::v1::NoopIsm;
            NoopIsm::decode(&*ism_any.value).ok().map(|ism| ism.id)
        } else if type_url.contains("AggregationISM") {
            use crate::proto::hyperlane::core::interchain_security::v1::AggregationIsm;
            AggregationIsm::decode(&*ism_any.value).ok().map(|ism| ism.id)
        } else {
            None
        }
    }

    /// Wait for a hyperlane ISM to be created by polling the hyperlane query service
    async fn wait_for_hyperlane_ism_creation(
        &self,
        ids_before: &std::collections::HashSet<String>,
        max_retries: u32,
    ) -> Result<String> {
        use crate::proto::hyperlane::core::interchain_security::v1::QueryIsmsRequest;

        info!("Waiting for hyperlane ISM creation. IDs before: {:?}", ids_before);

        for attempt in 0..max_retries {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            match self.hyperlane_isms(QueryIsmsRequest { pagination: None }).await {
                Ok(isms_after) => {
                    info!(
                        "Query returned {} ISMs (attempt {})",
                        isms_after.isms.len(),
                        attempt + 1
                    );

                    // Try to decode each ISM to get its ID
                    for (i, ism_any) in isms_after.isms.iter().enumerate() {
                        debug!("ISM {}: type_url = {}", i, ism_any.type_url);
                        if let Some(id) = self.decode_hyperlane_ism_id(ism_any) {
                            debug!("  Decoded ID: {}", id);
                            if !ids_before.contains(&id) {
                                info!(
                                    "Found newly created hyperlane ISM with ID: {} (attempt {})",
                                    id,
                                    attempt + 1
                                );
                                return Ok(id);
                            } else {
                                debug!("  ID was in the before set, skipping");
                            }
                        } else {
                            warn!("  Failed to decode ISM with type_url: {}", ism_any.type_url);
                        }
                    }
                    info!("Hyperlane ISM not found yet, attempt {}/{}", attempt + 1, max_retries);
                }
                Err(e) => {
                    warn!("Failed to query hyperlane ISMs on attempt {}: {}", attempt + 1, e);
                }
            }
        }

        Err(IsmClientError::TxFailed(format!(
            "Hyperlane ISM was not found after {} attempts",
            max_retries
        )))
    }
}
#[cfg(test)]
mod tests {
    use crate::message::{StateInclusionProofMsg, StateTransitionProofMsg};
    use prost::Message;

    use super::*;

    #[allow(dead_code)]
    fn create_test_config() -> ClientConfig {
        ClientConfig {
            grpc_endpoint: "http://localhost:9090".to_string(),
            private_key_hex: "0123456789abcdef".repeat(8), // 64 hex chars
            signer_address: String::new(),                 // Will be derived
            chain_id: "test-chain".to_string(),
            ism_id: String::new(),
            gas_price: 1000,
            max_gas: 200_000,
            confirmation_timeout: 30,
        }
    }

    #[test]
    fn test_state_transition_proof_message_structure() {
        // Test the new message structure based on actual Celestia PR #5788
        let proof_msg = StateTransitionProofMsg::new(
            "".to_string(),            // Empty ISM ID should be validated
            vec![1, 2, 3],             // proof
            vec![4, 5, 6],             // public_values
            "test_signer".to_string(), // signer
        );

        // Test the new field structure
        assert_eq!(proof_msg.id, "");
        assert_eq!(proof_msg.proof, vec![1, 2, 3]);
        assert_eq!(proof_msg.public_values, vec![4, 5, 6]);
        assert_eq!(proof_msg.signer, "test_signer");
    }

    #[test]
    fn test_state_inclusion_proof_message_structure() {
        // Test the new message structure based on actual Celestia PR #5790
        let proof_msg = StateInclusionProofMsg::new(
            "test-ism".to_string(),    // ISM ID
            vec![7, 8, 9],             // proof
            vec![10, 11, 12],          // public_values
            "test_signer".to_string(), // signer
        );

        // Test the new field structure
        assert_eq!(proof_msg.id, "test-ism");
        assert_eq!(proof_msg.proof, vec![7, 8, 9]);
        assert_eq!(proof_msg.public_values, vec![10, 11, 12]);
        assert_eq!(proof_msg.signer, "test_signer");
    }

    #[test]
    fn test_message_serialization() {
        let proof_msg = StateTransitionProofMsg::new(
            "test-ism-123".to_string(),
            vec![0xff, 0xee, 0xdd],
            vec![0x01, 0x02, 0x03],
            "test_signer".to_string(),
        );

        // Test that the message can be serialized (this validates the structure)
        let serialized = proof_msg.encode_to_vec();
        assert!(!serialized.is_empty());

        // Test deserialization
        let deserialized: StateTransitionProofMsg =
            StateTransitionProofMsg::decode(serialized.as_slice()).expect("failed to decode");

        assert_eq!(deserialized.id, proof_msg.id);
        assert_eq!(deserialized.proof, proof_msg.proof);
        assert_eq!(deserialized.public_values, proof_msg.public_values);
        assert_eq!(deserialized.signer, proof_msg.signer);
    }

    #[test]
    fn test_client_config_usage() {
        let config = create_test_config();

        // Test that config fields are accessible and properly structured
        assert_eq!(config.grpc_endpoint, "http://localhost:9090");
        assert_eq!(config.chain_id, "test-chain");
        assert_eq!(config.gas_price, 1000);
        assert_eq!(config.max_gas, 200_000);
        assert_eq!(config.confirmation_timeout, 30);

        // Test that private key is properly formatted (64 hex chars)
        assert_eq!(config.private_key_hex.len(), 128); // 64 bytes * 2 chars per byte
    }
}
