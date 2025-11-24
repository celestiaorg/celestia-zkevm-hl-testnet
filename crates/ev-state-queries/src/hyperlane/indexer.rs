use crate::DefaultProvider;
/// This service listens for Dispatch events emitted from the Mailbox contract
/// using the reth websocket.
/// Events are then processed and inserted into the storage (rocksDB)
use alloy_primitives::Address;
use alloy_provider::{Provider, WsConnect};
use alloy_rpc_types::{Filter, Log};
use alloy_sol_types::SolEvent;
use anyhow::Result;
use ev_zkevm_types::{
    events::{Dispatch, DispatchEvent},
    hyperlane::decode_hyperlane_message,
};
use std::{env, str::FromStr, sync::Arc};
use storage::hyperlane::{StoredHyperlaneMessage, message::HyperlaneMessageStore};
use tracing::debug;

/// HyperlaneIndexer is a service that indexes Hyperlane messages from the Dispatch event emitted from the Mailbox contract.
#[derive(Debug, Clone)]
pub struct HyperlaneIndexer {
    filter: Filter,
}

/// Implementation of the HyperlaneIndexer that queries the network for Dispatch messages.
impl HyperlaneIndexer {
    pub fn new(filter: Filter) -> Self {
        Self { filter }
    }

    /// Returns a clone of the base filter configured during construction.
    pub fn filter(&self) -> Filter {
        self.filter.clone()
    }

    /// Returns a filter scoped to the provided inclusive block range.
    pub fn filter_with_range(&self, from_block: u64, to_block: u64) -> Filter {
        self.filter().from_block(from_block).to_block(to_block)
    }

    pub async fn process(
        &self,
        filter: Filter,
        provider: DefaultProvider,
        store: Arc<HyperlaneMessageStore>,
    ) -> Result<()> {
        let logs = provider.get_logs(&filter).await?;
        for log in logs {
            Self::store_message(&log, &store)?;
        }

        Ok(())
    }

    fn store_message(log: &Log, store: &HyperlaneMessageStore) -> Result<()> {
        match Dispatch::decode_log_data(log.data()) {
            Ok(event) => {
                let dispatch_event: DispatchEvent = event.into();
                let current_index = store.current_index()?;
                let hyperlane_message =
                    decode_hyperlane_message(&dispatch_event.message).expect("Failed to decode Hyperlane message");
                let stored_message = StoredHyperlaneMessage::new(hyperlane_message, log.block_number);
                store.insert_message(current_index, stored_message)?;
                debug!("Inserted Hyperlane Message at index: {current_index}");
                Ok(())
            }
            Err(e) => Err(anyhow::anyhow!("Failed to decode Dispatch Event: {e:?}")),
        }
    }
}

impl Default for HyperlaneIndexer {
    fn default() -> Self {
        let contract_address = Address::from_str("0xb1c938f5ba4b3593377f399e12175e8db0c787ff").unwrap();
        let filter = Filter::new()
            .address(contract_address)
            .event(&Dispatch::id())
            .from_block(0)
            .to_block(10000);
        Self { filter }
    }
}
