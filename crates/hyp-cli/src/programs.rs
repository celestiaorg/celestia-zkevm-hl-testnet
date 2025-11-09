use sp1_sdk::include_elf;

/// The ELF for the Hyperlane message verification program
pub const EV_HYPERLANE_ELF: &[u8] = include_elf!("ev-hyperlane-program");

/// The ELF for the block range execution program
pub const EV_RANGE_EXEC_ELF: &[u8] = include_elf!("ev-range-exec-program");
