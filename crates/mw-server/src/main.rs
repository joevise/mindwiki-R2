//! mindwiki CLI 入口（Step0 骨架）

use anyhow::Result;

fn main() -> Result<()> {
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
        Some(other) => {
            eprintln!("unknown command: {other}");
            eprintln!("usage: mindwiki <version|skills>");
        }
        None => {
            println!("mindwiki {} — 企业级安全 AI 知识库", env!("CARGO_PKG_VERSION"));
            println!("commands: version | skills");
        }
    }
    Ok(())
}
