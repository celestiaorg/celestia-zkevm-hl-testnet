//! Celestia gRPC Client for x/zkism
//!
//! This crate provides a gRPC client for the x/zkism Celestia module.
//! In supports transaction submission for both state transition and state inclusion proofs
//! as well as gRPC module queries.

pub mod client;
pub mod error;
pub mod message;
pub mod proto;
pub mod proto_ext;
pub mod types;

pub use client::CelestiaIsmClient;
pub use error::{IsmClientError, Result};
pub use message::{StateInclusionProofMsg, StateTransitionProofMsg};

// Re-export Celestia zkism messages
pub use proto::celestia::zkism::v1::{
    MsgCreateZkExecutionIsm, MsgSubmitMessages, MsgSubmitMessagesResponse, MsgUpdateZkExecutionIsm,
    MsgUpdateZkExecutionIsmResponse, QueryIsmRequest,
};

// Re-export Hyperlane core messages
pub use proto::hyperlane::core::interchain_security::v1::{
    MsgAnnounceValidator, MsgAnnounceValidatorResponse, MsgCreateMerkleRootMultisigIsm,
    MsgCreateMerkleRootMultisigIsmResponse, MsgCreateNoopIsm, MsgCreateNoopIsmResponse,
};
pub use proto::hyperlane::core::post_dispatch::v1::{
    MsgCreateMerkleTreeHook, MsgCreateMerkleTreeHookResponse, MsgCreateNoopHook, MsgCreateNoopHookResponse,
};
pub use proto::hyperlane::core::v1::{
    MsgCreateMailbox, MsgCreateMailboxResponse, MsgProcessMessage, MsgSetMailbox, MsgSetMailboxResponse,
};

// Re-export Hyperlane warp messages
pub use proto::hyperlane::warp::v1::{
    MsgCreateCollateralToken, MsgCreateCollateralTokenResponse, MsgEnrollRemoteRouter, MsgEnrollRemoteRouterResponse,
    MsgRemoteTransfer, MsgSetToken, MsgSetTokenResponse, RemoteRouter,
};

pub use types::TxResponse;
