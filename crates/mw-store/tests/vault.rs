//! Step 2 验收测试：加密容器 + Git 版本管理 + 内存解密。

use mw_crypto::KeyGateway;
use mw_store::{container, Vault};

fn read_container(vault: &Vault) -> container::ContainerData {
    let data = std::fs::read(vault.container_path()).unwrap();
    container::decode(&data).unwrap()
}

fn reopen_gateway(vault: &Vault, password: &str) -> KeyGateway {
    let c = read_container(vault);
    let gw = KeyGateway::from_container(c.salt.clone(), c.verify_token.clone());
    gw.open(password).unwrap();
    gw
}

#[test]
fn vault_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = Vault::open(tmp.path()).unwrap();

    let gw = KeyGateway::new().unwrap();
    vault.init(&gw, "pw-A").unwrap();
    assert!(vault.exists());

    {
        let session = vault.open_session(&gw).unwrap();
        std::fs::write(session.work_dir().join("secret.md"), "TOPSECRET v1").unwrap();
        std::fs::create_dir_all(session.work_dir().join("notes")).unwrap();
        std::fs::write(session.work_dir().join("notes/a.md"), "note A").unwrap();
        session.git_commit("add secret").unwrap();
        vault.seal_session(&gw, &session).unwrap();
    }
    gw.close();

    // 第二个会话：改动 + 再次提交
    let gw2 = reopen_gateway(&vault, "pw-A");
    {
        let session = vault.open_session(&gw2).unwrap();
        std::fs::write(session.work_dir().join("secret.md"), "TOPSECRET v2").unwrap();
        session.git_commit("update secret").unwrap();
        vault.seal_session(&gw2, &session).unwrap();
    }
    gw2.close();

    // 重开验证：文件内容 + git 历史完整
    let gw3 = reopen_gateway(&vault, "pw-A");
    {
        let session = vault.open_session(&gw3).unwrap();
        let content = std::fs::read_to_string(session.work_dir().join("secret.md")).unwrap();
        assert_eq!(content, "TOPSECRET v2");
        assert_eq!(
            std::fs::read_to_string(session.work_dir().join("notes/a.md")).unwrap(),
            "note A"
        );
        let log = session.git_log().unwrap();
        assert_eq!(log.len(), 2);
        assert!(log[0].contains("update secret"));
        assert!(log[1].contains("add secret"));
    }
    gw3.close();
}

#[test]
fn wrong_password_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = Vault::open(tmp.path()).unwrap();
    let gw = KeyGateway::new().unwrap();
    vault.init(&gw, "pw-A").unwrap();
    gw.close();

    let c = read_container(&vault);
    let bad = KeyGateway::from_container(c.salt.clone(), c.verify_token.clone());
    assert!(bad.open("pw-B").is_err());
    assert_eq!(bad.state(), mw_crypto::GatewayState::Closed);
}

#[test]
fn disk_is_ciphertext() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = Vault::open(tmp.path()).unwrap();
    let gw = KeyGateway::new().unwrap();
    vault.init(&gw, "pw-A").unwrap();
    {
        let session = vault.open_session(&gw).unwrap();
        std::fs::write(session.work_dir().join("secret.md"), "TOPSECRET").unwrap();
        session.git_commit("add secret").unwrap();
        vault.seal_session(&gw, &session).unwrap();
    }
    let data = std::fs::read(vault.container_path()).unwrap();
    assert!(!data.windows(9).any(|w| w == b"TOPSECRET"));
    assert!(!data.windows(9).any(|w| w == b"secret.md"));
}
