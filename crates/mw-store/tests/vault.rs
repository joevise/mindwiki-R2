//! Step 2 验收测试：加密容器 + Git 版本管理 + 内存解密。

use mw_crypto::KeyGateway;
use std::sync::Arc;
use mw_store::{container, Vault};

fn read_container(vault: &Vault) -> container::ContainerData {
    let data = std::fs::read(vault.container_path()).unwrap();
    container::decode(&data).unwrap()
}

fn reopen_gateway(vault: &Vault, password: &str) -> Arc<KeyGateway> {
    let c = read_container(vault);
    let gw = Arc::new(KeyGateway::from_container(c.salt.clone(), c.verify_token.clone()));
    gw.open(password).unwrap();
    gw
}

#[test]
fn vault_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = Vault::open(tmp.path()).unwrap();

    let gw = Arc::new(KeyGateway::new().unwrap());
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
    let gw = Arc::new(KeyGateway::new().unwrap());
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
    let gw = Arc::new(KeyGateway::new().unwrap());
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

#[test]
fn close_terminates_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = Vault::open(tmp.path()).unwrap();
    let gw = Arc::new(KeyGateway::new().unwrap());
    vault.init(&gw, "pw-A").unwrap();

    let session = vault.open_session(&gw).unwrap();
    assert!(!session.is_terminated());
    assert_eq!(gw.active_sessions(), 1);
    assert!(gw.closed_at.lock().unwrap().is_none());

    gw.close();

    // 终止旗标置位；注册表清空；审计时间已记录
    assert!(session.is_terminated());
    assert_eq!(gw.active_sessions(), 0);
    assert!(gw.closed_at.lock().unwrap().is_some());

    // 被终止的会话 seal 被拒绝；容器保持旧密文
    let before = std::fs::read(vault.container_path()).unwrap();
    assert!(vault.seal_session(&gw, &session).is_err());
    let after = std::fs::read(vault.container_path()).unwrap();
    assert_eq!(before, after);
}

#[test]
fn terminated_session_not_sealed() {
    let tmp = tempfile::tempdir().unwrap();
    let vault = Vault::open(tmp.path()).unwrap();
    let gw = Arc::new(KeyGateway::new().unwrap());
    vault.init(&gw, "pw-A").unwrap();

    // 先封印一版 v1
    {
        let session = vault.open_session(&gw).unwrap();
        std::fs::write(session.work_dir().join("secret.md"), "v1").unwrap();
        session.git_commit("v1").unwrap();
        vault.seal_session(&gw, &session).unwrap();
    }

    // 第二个会话写入 v2 但未 seal，就被 close 终止
    let work_dir;
    {
        let session = vault.open_session(&gw).unwrap();
        work_dir = session.work_dir().to_path_buf();
        std::fs::write(session.work_dir().join("secret.md"), "v2-leaked").unwrap();
        gw.close();
        assert!(session.is_terminated());
    }
    // drop 后临时目录已销毁，明文不落盘
    assert!(!work_dir.exists());

    // 重开验证：容器仍是 v1，v2 从未写入
    let gw2 = reopen_gateway(&vault, "pw-A");
    let session = vault.open_session(&gw2).unwrap();
    assert_eq!(
        std::fs::read_to_string(session.work_dir().join("secret.md")).unwrap(),
        "v1"
    );
    gw2.close();
}

#[test]
fn admin_token_file_created_on_init() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let vault = Vault::open(tmp.path()).unwrap();
    let gw = Arc::new(KeyGateway::new().unwrap());
    vault.init(&gw, "pw-A").unwrap();

    let path = vault.admin_token_path();
    assert!(path.exists());
    let token = std::fs::read_to_string(&path).unwrap();
    assert_eq!(token.trim().len(), 64);
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);

    // 幂等：再次取返回同一 token
    assert_eq!(vault.ensure_admin_token().unwrap(), token.trim());
    gw.close();
}
