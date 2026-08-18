//! mindwiki CLI 入口

use anyhow::{anyhow, Result};
use std::path::PathBuf;

/// skills 目录解析：MW_SKILLS_ROOT 环境变量 > 可执行文件上两级（target/<profile> 之外）> ./skills
fn skills_root() -> PathBuf {
    if let Ok(p) = std::env::var("MW_SKILLS_ROOT") {
        return PathBuf::from(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(root) = exe.parent().and_then(|p| p.parent()).and_then(|p| p.parent()) {
            let candidate = root.join("skills");
            if candidate.is_dir() {
                return candidate;
            }
        }
    }
    PathBuf::from("skills")
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("version") | Some("--version") => {
            println!("mindwiki {}", env!("CARGO_PKG_VERSION"));
        }
        Some("skills") => {
            let loader = mw_wiki::SkillLoader::new("skills");
            for s in loader.load_all()? {
                println!("  {} — {}", s.name, s.path.display());
            }
        }
        Some("init") => {
            let password = flag_value(&args, "--password")
                .ok_or_else(|| anyhow!("usage: mindwiki init --password ***"))?;
            let vault = mw_store::Vault::open(std::env::current_dir()?)?;
            let gateway = mw_crypto::KeyGateway::new()?;
            vault.init(&gateway, &password)?;
            let token = vault.ensure_admin_token()?;
            println!("vault initialized: {}", vault.container_path().display());
            println!("admin token (save it, required for remote close): {token}");
            println!("token file: {} (chmod 600)", vault.admin_token_path().display());
        }
        Some("serve") => {
            let vault = mw_store::Vault::open(std::env::current_dir()?)?;
            if !vault.exists() {
                return Err(anyhow!("no vault in current directory (run: mindwiki init --password ***)"));
            }
            let state = mw_server::serve::load_state(&vault)?;
            let local = tokio::net::TcpListener::bind("127.0.0.1:7900").await?;
            println!("serving gateway API on http://127.0.0.1:7900");
            match flag_value(&args, "--admin-bind") {
                Some(bind) => {
                    let admin = tokio::net::TcpListener::bind(&bind).await?;
                    println!("remote admin enabled on http://{bind} (protect with admin token)");
                    tokio::try_join!(
                        mw_server::serve::serve(local, state.clone()),
                        mw_server::serve::serve(admin, state)
                    )?;
                }
                None => {
                    mw_server::serve::serve(local, state).await?;
                }
            }
        }
        Some("lock") => {
            let vault = mw_store::Vault::open(std::env::current_dir()?)?;
            let path = vault.admin_token_path();
            let token = std::fs::read_to_string(&path)
                .map_err(|_| anyhow!("no admin.token — run: mindwiki init --password ***"))?;
            mw_server::serve::remote_close("127.0.0.1:7900", token.trim()).await?;
            println!("gateway locked: sessions terminated, keys zeroized");
        }
        Some("remote-close") => {
            let host = flag_value(&args, "--host")
                .ok_or_else(|| anyhow!("usage: mindwiki remote-close --host x.x.x.x:7901 --token ***"))?;
            let token = flag_value(&args, "--token")
                .ok_or_else(|| anyhow!("usage: mindwiki remote-close --host x.x.x.x:7901 --token ***"))?;
            mw_server::serve::remote_close(&host, &token).await?;
            println!("remote gateway at {host} locked");
        }
        Some("status") => {
            let vault = mw_store::Vault::open(std::env::current_dir()?)?;
            if !vault.exists() {
                println!("no vault in current directory (run: mindwiki init --password ***)");
            } else {
                let data = std::fs::read(vault.container_path())?;
                let (version, _) = mw_store::container::validate(&data)?;
                println!("vault:   {}", vault.container_path().display());
                println!("size:    {} bytes", data.len());
                println!("version: {version}");
                println!("state:   sealed (ciphertext at rest)");
            }
        }
        Some("ask") => {
            let question = args
                .get(2)
                .ok_or_else(|| anyhow!("usage: mindwiki ask \"问题\""))?;
            let work_dir = std::env::current_dir()?;
            let agent = mw_agent::WikiAgent::new(skills_root(), &work_dir);
            let reply = agent.ask(question).await?;
            println!("{reply}");
        }
        Some(other) => {
            eprintln!("unknown command: {other}");
            eprintln!("usage: mindwiki <version|skills|init|status|ask|serve|lock|remote-close>");
        }
        None => {
            println!("mindwiki {} — 企业级安全 AI 知识库", env!("CARGO_PKG_VERSION"));
            println!("commands: version | skills | init | status | ask | serve | lock | remote-close");
        }
    }
    Ok(())
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
