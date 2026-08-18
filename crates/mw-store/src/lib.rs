//! # mw-store — L0 数据层
//!
//! 加密容器 + Git 版本管理 + 原子写。
//! 磁盘上永远只有密文；明文只存在于受控解密会话（临时目录）。

pub mod container;
pub mod vault;

pub use vault::Vault;
