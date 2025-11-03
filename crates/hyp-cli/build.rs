use sp1_build::{build_program_with_args, BuildArgs};

fn main() {
    // Build the SP1 programs needed for ZK ISM creation
    build_program_with_args(
        "../sp1/ev-hyperlane/program",
        BuildArgs {
            ignore_rust_version: true,
            ..Default::default()
        },
    );

    build_program_with_args(
        "../sp1/ev-range-exec/program",
        BuildArgs {
            ignore_rust_version: true,
            ..Default::default()
        },
    );
}
