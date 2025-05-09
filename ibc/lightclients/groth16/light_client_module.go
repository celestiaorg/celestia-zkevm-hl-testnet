package groth16

import (
	"fmt"

	errorsmod "cosmossdk.io/errors"
	"github.com/cosmos/cosmos-sdk/codec"
	sdktypes "github.com/cosmos/cosmos-sdk/types"
	clienttypes "github.com/cosmos/ibc-go/v10/modules/core/02-client/types"
	ibcerrors "github.com/cosmos/ibc-go/v10/modules/core/errors"
	"github.com/cosmos/ibc-go/v10/modules/core/exported"
)

var _ exported.LightClientModule = (*LightClientModule)(nil)

// LightClientModule implements the core IBC api.LightClientModule interface.
type LightClientModule struct {
	cdc           codec.BinaryCodec
	storeProvider clienttypes.StoreProvider
}

// NewLightClientModule returns a new groth16 LightClientModule.
func NewLightClientModule(cdc codec.BinaryCodec, storeProvider clienttypes.StoreProvider) LightClientModule {
	return LightClientModule{
		cdc:           cdc,
		storeProvider: storeProvider,
	}
}

// Initialize unmarshals the provided client and consensus states and performs basic validation. It calls into the
// clientState.initialize method.
func (l LightClientModule) Initialize(ctx sdktypes.Context, clientID string, clientStateBz []byte, consensusStateBz []byte) error {
	var clientState ClientState
	if err := l.cdc.Unmarshal(clientStateBz, &clientState); err != nil {
		return fmt.Errorf("failed to unmarshal client state bytes into client state: %w", err)
	}

	if err := clientState.Validate(); err != nil {
		return err
	}

	var consensusState ConsensusState
	if err := l.cdc.Unmarshal(consensusStateBz, &consensusState); err != nil {
		return fmt.Errorf("failed to unmarshal consensus state bytes into consensus state: %w", err)
	}

	if err := consensusState.ValidateBasic(); err != nil {
		return err
	}

	clientStore := l.storeProvider.ClientStore(ctx, clientID)
	return clientState.initialize(ctx, l.cdc, clientStore, &consensusState)
}

// VerifyClientMessage obtains the client state associated with the client
// identifier and calls into the clientState.VerifyClientMessage method.
func (l LightClientModule) VerifyClientMessage(ctx sdktypes.Context, clientID string, clientMsg exported.ClientMessage) error {
	clientStore := l.storeProvider.ClientStore(ctx, clientID)
	clientState, found := getClientState(clientStore, l.cdc)
	if !found {
		return errorsmod.Wrap(clienttypes.ErrClientNotFound, clientID)
	}

	return clientState.VerifyClientMessage(ctx, l.cdc, clientStore, clientMsg)
}

// CheckForMisbehaviour is a no-op.
func (l LightClientModule) CheckForMisbehaviour(ctx sdktypes.Context, clientID string, clientMsg exported.ClientMessage) bool {
	return false
}

// UpdateStateOnMisbehaviour is a no-op.
func (l LightClientModule) UpdateStateOnMisbehaviour(ctx sdktypes.Context, clientID string, clientMsg exported.ClientMessage) {
}

// UpdateState obtains the client state associated with the client identifier and calls into the clientState.UpdateConsensusState method.
func (l LightClientModule) UpdateState(ctx sdktypes.Context, clientID string, clientMsg exported.ClientMessage) []exported.Height {
	clientStore := l.storeProvider.ClientStore(ctx, clientID)
	clientState, found := getClientState(clientStore, l.cdc)
	if !found {
		panic(errorsmod.Wrap(clienttypes.ErrClientNotFound, clientID))
	}

	height, err := clientState.UpdateState(ctx, l.cdc, clientStore, clientMsg)
	if err != nil {
		panic(err)
	}

	return height
}

// VerifyMembership obtains the client state associated with the client identifier and calls into the clientState.verifyMembership method.
func (l LightClientModule) VerifyMembership(
	ctx sdktypes.Context,
	clientID string,
	height exported.Height,
	delayTimePeriod uint64,
	delayBlockPeriod uint64,
	proof []byte,
	path exported.Path,
	value []byte,
) error {
	clientStore := l.storeProvider.ClientStore(ctx, clientID)
	clientState, found := getClientState(clientStore, l.cdc)
	if !found {
		return errorsmod.Wrap(clienttypes.ErrClientNotFound, clientID)
	}

	return clientState.verifyMembership(ctx, clientStore, l.cdc, height, delayTimePeriod, delayBlockPeriod, proof, path, value)
}

// VerifyNonMembership obtains the client state associated with the client
// identifier and calls into the clientState.verifyNonMembership method.
func (l LightClientModule) VerifyNonMembership(
	ctx sdktypes.Context,
	clientID string,
	height exported.Height,
	delayTimePeriod uint64,
	delayBlockPeriod uint64,
	proof []byte,
	path exported.Path,
) error {
	clientStore := l.storeProvider.ClientStore(ctx, clientID)
	clientState, found := getClientState(clientStore, l.cdc)
	if !found {
		return errorsmod.Wrap(clienttypes.ErrClientNotFound, clientID)
	}

	return clientState.verifyNonMembership(ctx, clientStore, l.cdc, height, delayTimePeriod, delayBlockPeriod, proof, path)
}

// Status obtains the client state associated with the client identifier and
// calls into the clientState.status method.
func (l LightClientModule) Status(ctx sdktypes.Context, clientID string) exported.Status {
	clientStore := l.storeProvider.ClientStore(ctx, clientID)
	clientState, found := getClientState(clientStore, l.cdc)
	if !found {
		return exported.Unknown
	}

	return clientState.status(ctx, clientStore, l.cdc)
}

// LatestHeight returns the latest height for the client state for the given client identifier.
// If no client is present for the provided client identifier a zero value height is returned.
func (l LightClientModule) LatestHeight(ctx sdktypes.Context, clientID string) exported.Height {
	clientStore := l.storeProvider.ClientStore(ctx, clientID)
	clientState, found := getClientState(clientStore, l.cdc)
	if !found {
		return clienttypes.ZeroHeight()
	}

	return clientState.GetLatestClientHeight()
}

// TimestampAtHeight returns the timestamp at height.
func (l LightClientModule) TimestampAtHeight(ctx sdktypes.Context, clientID string, height exported.Height) (uint64, error) {
	clientStore := l.storeProvider.ClientStore(ctx, clientID)
	clientState, found := getClientState(clientStore, l.cdc)
	if !found {
		return 0, errorsmod.Wrap(clienttypes.ErrClientNotFound, clientID)
	}

	return clientState.getTimestampAtHeight(clientStore, l.cdc, height)
}

// RecoverClient asserts that the substitute client is a tendermint client. It obtains the client state associated with the
// subject client and calls into the subjectClientState.CheckSubstituteAndUpdateState method.
func (l LightClientModule) RecoverClient(ctx sdktypes.Context, clientID, substituteClientID string) error {
	substituteClientType, _, err := clienttypes.ParseClientIdentifier(substituteClientID)
	if err != nil {
		return err
	}

	if substituteClientType != exported.Tendermint {
		return errorsmod.Wrapf(clienttypes.ErrInvalidClientType, "expected: %s, got: %s", exported.Tendermint, substituteClientType)
	}

	clientStore := l.storeProvider.ClientStore(ctx, clientID)
	clientState, found := getClientState(clientStore, l.cdc)
	if !found {
		return errorsmod.Wrap(clienttypes.ErrClientNotFound, clientID)
	}

	substituteClientStore := l.storeProvider.ClientStore(ctx, substituteClientID)
	substituteClient, found := getClientState(substituteClientStore, l.cdc)
	if !found {
		return errorsmod.Wrap(clienttypes.ErrClientNotFound, substituteClientID)
	}

	return clientState.CheckSubstituteAndUpdateState(ctx, l.cdc, clientStore, substituteClientStore, substituteClient)
}

// VerifyUpgradeAndUpdateState obtains the client state associated with the client identifier and calls into the clientState.VerifyUpgradeAndUpdateState method.
// The new client and consensus states will be unmarshaled and an error is returned if the new client state is not at a height greater
// than the existing client.
func (l LightClientModule) VerifyUpgradeAndUpdateState(
	ctx sdktypes.Context,
	clientID string,
	newClient []byte,
	newConsState []byte,
	upgradeClientProof,
	upgradeConsensusStateProof []byte,
) error {
	var newClientState ClientState
	if err := l.cdc.Unmarshal(newClient, &newClientState); err != nil {
		return errorsmod.Wrap(clienttypes.ErrInvalidClient, err.Error())
	}

	var newConsensusState ConsensusState
	if err := l.cdc.Unmarshal(newConsState, &newConsensusState); err != nil {
		return errorsmod.Wrap(clienttypes.ErrInvalidConsensus, err.Error())
	}

	clientStore := l.storeProvider.ClientStore(ctx, clientID)
	clientState, found := getClientState(clientStore, l.cdc)
	if !found {
		return errorsmod.Wrap(clienttypes.ErrClientNotFound, clientID)
	}

	// last height of current counterparty chain must be client's latest height
	lastHeight := clientState.GetLatestClientHeight()
	if !newClientState.GetLatestClientHeight().GT(lastHeight) {
		return errorsmod.Wrapf(ibcerrors.ErrInvalidHeight, "upgraded client height %d must be at greater than current client height %d", newClientState.LatestHeight, lastHeight)
	}

	return clientState.VerifyUpgradeAndUpdateState(ctx, l.cdc, clientStore, &newClientState, &newConsensusState, upgradeClientProof, upgradeConsensusStateProof)
}
