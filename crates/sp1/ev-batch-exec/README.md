## Overview

## Program Inputs
| Name | Type | Description |
|---|---|---|
| blocks | Vec<BlockExecInput> | Inputs required to execute a range of blocks |


Note that by design proofs are written separately in recursive circuits like this one.
We use the standard SP1 approach to write compressed proofs generated using `ev-exec`.

## Program Outputs
These are the same as for `ev-range-exec`, because the circuits fulfill a similar purpose and update the same ISM endpoint. The only difference is that `batch-exec` does not prove every single block recursively, but instead takes a block range as input and generates a single proof using just one prover instance.

### State: Encapsulate the input and output for each state transition step
| Name | Type | Description |
|---|---|---|
| celestia_header_hash | [u8;32] | the new Celestia header hash after applying the blocks |
| celestia_height | u64 | the new Celestia height after applying the blocks |
| trusted_height | u64 | the trusted EV height in the ISM | 
| trusted_state_root | [u8;32] | the trusted EV state root in the ISM |
| height | u64 | the new EV height after applying the blocks |
| state_root | [u8;32] | the new EV state root after applying the blocks |
| namespace | [u8;29] | the Celestia namespace that contains the data blobs which themselves contain EV blocks | |
| public_key | [u8;32] | the sequencer's public key for verification |

### BlockRangeExecOutput: Circuit outputs used for on-chain verification
| Name | Type | Description |
|---|---|---|
| state_len_bytes | [u8;8] | Little endian encoded bytes of the length of the State |
| state | State | The starting point of the state transition
| new_state_len_bytes | [u8;8] | Little endian encoded bytes of the length of the new State |
| new_state | State | The result of the state transition