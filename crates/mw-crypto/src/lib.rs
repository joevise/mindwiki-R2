//! # mw-crypto — L1 安全层
//!
//! 密钥闸门（Key Gateway）+ 加密接口。
//! 密钥只在内存（zeroize），一键关闭即清零，此后一切密文为数学噪声。

pub mod gateway;
pub mod secret;

pub use gateway::{GatewayState, KeyGateway};
pub use secret::SecretStore;
