//! Extension traits for generated proto types
//!
//! Implements prost::Name for generated Hyperlane message types.
//! IntoProtobufAny is automatically implemented via blanket impl in celestia_types.

use prost::Name;

use crate::proto::hyperlane::core::interchain_security::v1::{
    MsgAnnounceValidator, MsgCreateMerkleRootMultisigIsm, MsgCreateNoopIsm,
};
use crate::proto::hyperlane::core::post_dispatch::v1::{MsgCreateMerkleTreeHook, MsgCreateNoopHook};
use crate::proto::hyperlane::core::v1::{MsgCreateMailbox, MsgSetMailbox};
use crate::proto::hyperlane::warp::v1::{MsgCreateCollateralToken, MsgEnrollRemoteRouter, MsgSetToken};

// ============================================================================
// prost::Name implementations
// ============================================================================

impl Name for MsgCreateNoopIsm {
    const NAME: &'static str = "MsgCreateNoopIsm";
    const PACKAGE: &'static str = "hyperlane.core.interchain_security.v1";
}

impl Name for MsgCreateMerkleRootMultisigIsm {
    const NAME: &'static str = "MsgCreateMerkleRootMultisigIsm";
    const PACKAGE: &'static str = "hyperlane.core.interchain_security.v1";
}

impl Name for MsgAnnounceValidator {
    const NAME: &'static str = "MsgAnnounceValidator";
    const PACKAGE: &'static str = "hyperlane.core.interchain_security.v1";
}

impl Name for MsgCreateNoopHook {
    const NAME: &'static str = "MsgCreateNoopHook";
    const PACKAGE: &'static str = "hyperlane.core.post_dispatch.v1";
}

impl Name for MsgCreateMerkleTreeHook {
    const NAME: &'static str = "MsgCreateMerkleTreeHook";
    const PACKAGE: &'static str = "hyperlane.core.post_dispatch.v1";
}

impl Name for MsgCreateMailbox {
    const NAME: &'static str = "MsgCreateMailbox";
    const PACKAGE: &'static str = "hyperlane.core.v1";
}

impl Name for MsgSetMailbox {
    const NAME: &'static str = "MsgSetMailbox";
    const PACKAGE: &'static str = "hyperlane.core.v1";
}

impl Name for MsgCreateCollateralToken {
    const NAME: &'static str = "MsgCreateCollateralToken";
    const PACKAGE: &'static str = "hyperlane.warp.v1";
}

impl Name for MsgSetToken {
    const NAME: &'static str = "MsgSetToken";
    const PACKAGE: &'static str = "hyperlane.warp.v1";
}

impl Name for MsgEnrollRemoteRouter {
    const NAME: &'static str = "MsgEnrollRemoteRouter";
    const PACKAGE: &'static str = "hyperlane.warp.v1";
}
