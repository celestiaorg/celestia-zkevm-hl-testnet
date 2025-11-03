use serde::{Deserialize, Serialize};

pub const APP_HOME: &str = ".hyp-cli";
pub const CONFIG_DIR: &str = "config";
pub const CONFIG_FILE: &str = "config.yaml";

pub const GROTH16_VK: &[u8] = include_bytes!("../resources/groth16_vk.bin");

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Config {
    pub celestia_rpc: String,
    pub evm_rpc: String,
    pub namespace_hex: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            celestia_rpc: "127.0.0.1:26658".to_string(),
            evm_rpc: "http://127.0.0.1:8545".to_string(),
            namespace_hex: "a8045f161bf468bf4d44".to_string(),
        }
    }
}
