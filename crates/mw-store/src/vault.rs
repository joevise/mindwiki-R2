//! Vault：一个知识库 = 一个加密容器 + 解密会话管理。
//! 磁盘上永远密文；明文只在受控临时目录，会话结束（Drop）即焚。

use crate::container;
use anyhow::{bail, Context, Result};
use mw_crypto::{KeyGateway, SessionRegistry};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 磁盘上的 vault（永远密文）
pub struct Vault {
    pub root: PathBuf,
}

/// 解密会话（受控临时目录，drop 时销毁）。
/// 注册到 gateway 的会话注册表；被终止（terminate=true）时 drop 不做 seal、直接销毁。
/// 注册表以 Arc 持有：会话可长驻于 AppState，不借用 gateway 生命周期。
pub struct DecryptedSession {
    tmp: tempfile::TempDir,
    session_id: String,
    terminate: Arc<AtomicBool>,
    registry: Option<Arc<dyn SessionRegistry>>,
}

impl DecryptedSession {
    pub fn work_dir(&self) -> &Path {
        self.tmp.path()
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// 是否已被 gateway 强制终止
    pub fn is_terminated(&self) -> bool {
        self.terminate.load(Ordering::SeqCst)
    }

    /// 在解密环境里做一次 git commit（没有 repo 则 init）。无变更则跳过。
    pub fn git_commit(&self, message: &str) -> Result<()> {
        let dir = self.work_dir();
        let repo = match git2::Repository::open(dir) {
            Ok(r) => r,
            Err(_) => git2::Repository::init(dir)?,
        };
        let mut index = repo.index()?;
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)?;
        index.write()?;
        let tree_oid = index.write_tree()?;
        let tree = repo.find_tree(tree_oid)?;
        let sig = git2::Signature::now("mindwiki", "mindwiki@local")?;
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        if let Some(p) = &parent {
            if p.tree_id() == tree_oid {
                return Ok(());
            }
        }
        let parents: Vec<&git2::Commit> = parent.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)?;
        Ok(())
    }

    /// git 提交历史（新→旧，"oid message"）
    pub fn git_log(&self) -> Result<Vec<String>> {
        let repo = git2::Repository::open(self.work_dir())?;
        let mut walk = repo.revwalk()?;
        walk.push_head()?;
        let mut out = Vec::new();
        for oid in walk {
            let c = repo.find_commit(oid?)?;
            out.push(format!("{} {}", c.id(), c.message().unwrap_or("").trim()));
        }
        Ok(out)
    }

    /// 从注册表注销（幂等）
    fn unregister(&self) {
        if let Some(reg) = &self.registry {
            reg.unregister(&self.session_id);
        }
    }
}

impl Drop for DecryptedSession {
    /// 被终止的会话：不 seal、直接销毁临时目录（数据丢弃，容器保持旧状态）。
    /// 正常会话：注销后销毁临时目录（TempDir drop 删除明文）。
    fn drop(&mut self) {
        if self.is_terminated() {
            tracing::warn!(session = %self.session_id, "terminated session destroyed without seal");
        }
        self.unregister();
    }
}

impl Vault {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        Ok(Self { root: root.as_ref().to_path_buf() })
    }

    pub fn container_path(&self) -> PathBuf {
        self.root.join("vault.mwenc")
    }

    pub fn exists(&self) -> bool {
        self.container_path().exists()
    }

    /// 初始化：创建 vault.mwenc（空 tar 加密 + 验证令牌）+ admin.token（远程关闭管理密钥）
    pub fn init(&self, gateway: &KeyGateway, password: &str) -> Result<()> {
        if self.exists() {
            bail!("vault already exists at {}", self.container_path().display());
        }
        fs::create_dir_all(&self.root)?;
        gateway.open(password)?;
        let empty = tempfile::TempDir::new()?;
        let tar = pack_dir(empty.path())?;
        let payload = gateway.encrypt(&tar)?;
        let token = gateway.ensure_verify_token()?;
        let data = container::encode(gateway.salt(), &token, &payload);
        atomic_write(&self.container_path(), &data)?;
        self.ensure_admin_token()?;
        Ok(())
    }

    pub fn admin_token_path(&self) -> PathBuf {
        self.root.join("admin.token")
    }

    /// 取 admin token；不存在则生成随机 token 写入（chmod 600）。客户保存，远程关闭必带。
    pub fn ensure_admin_token(&self) -> Result<String> {
        let path = self.admin_token_path();
        if path.exists() {
            return Ok(fs::read_to_string(&path)?.trim().to_string());
        }
        fs::create_dir_all(&self.root)?;
        let mut bytes = [0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
        let token = hex_encode(&bytes);
        write_private(&path, token.as_bytes())?;
        Ok(token)
    }

    /// 打开解密会话：解密容器 → 展开 tar 到受控临时目录 → 注册到 gateway 会话表
    pub fn open_session(&self, gateway: &Arc<KeyGateway>) -> Result<DecryptedSession> {
        gateway.guard()?;
        let data = fs::read(self.container_path()).context("read vault container")?;
        let c = container::decode(&data)?;
        let tar = gateway.decrypt(&c.payload)?;
        let tmp = tempfile::TempDir::new()?;
        unpack_dir(&tar, tmp.path())?;
        let session_id = new_session_id();
        let terminate = Arc::new(AtomicBool::new(false));
        gateway.register(&session_id, tmp.path(), terminate.clone());
        Ok(DecryptedSession {
            tmp,
            session_id,
            terminate,
            registry: Some(gateway.clone()),
        })
    }

    /// 封印：tar 打包 work_dir（含 .git）→ 加密 → 原子重写容器 → 从注册表注销。
    /// 被终止的会话拒绝 seal（容器保持旧密文）。
    pub fn seal_session(&self, gateway: &KeyGateway, session: &DecryptedSession) -> Result<()> {
        if session.is_terminated() {
            bail!("session terminated by gateway — seal refused, container untouched");
        }
        gateway.guard()?;
        let tar = pack_dir(session.work_dir())?;
        let payload = gateway.encrypt(&tar)?;
        let token = gateway.ensure_verify_token()?;
        let data = container::encode(gateway.salt(), &token, &payload);
        atomic_write(&self.container_path(), &data)?;
        session.unregister();
        Ok(())
    }
}

fn new_session_id() -> String {
    let mut bytes = [0u8; 16];
    rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut bytes);
    hex_encode(&bytes)
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// 私钥文件写入：创建即 0600（unix），已存在则覆盖并保持私有权限
fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, data))?;
    }
    #[cfg(not(unix))]
    {
        fs::write(path, data)?;
    }
    Ok(())
}

/// 目录 → tar.gz（含点文件，如 .git）
fn pack_dir(dir: &Path) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    {
        let gz = flate2::write::GzEncoder::new(&mut buf, flate2::Compression::default());
        let mut builder = tar::Builder::new(gz);
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let path = entry.path();
            if entry.file_type()?.is_dir() {
                builder.append_dir_all(name, path)?;
            } else {
                builder.append_path_with_name(path, name)?;
            }
        }
        builder.into_inner()?.finish()?;
    }
    Ok(buf)
}

/// tar.gz → 目录
fn unpack_dir(data: &[u8], dir: &Path) -> Result<()> {
    let gz = flate2::read::GzDecoder::new(data);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(dir)?;
    Ok(())
}

/// 原子写：同目录临时文件 + rename
fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("mwenc.tmp");
    fs::write(&tmp, data)?;
    fs::rename(&tmp, path)?;
    Ok(())
}
