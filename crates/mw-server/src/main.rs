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
            println!("vault initialized: {}", vault.container_path().display());
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
            eprintln!("usage: mindwiki <version|skills|init|status|ask>");
        }
        None => {
            println!("mindwiki {} — 企业级安全 AI 知识库", env!("CARGO_PKG_VERSION"));
            println!("commands: version | skills | init | status | ask");
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
