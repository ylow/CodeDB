use std::path::PathBuf;
use anyhow::Result;
use clap::{Parser, Subcommand};
use codedb_core::CodeDB;

#[derive(Parser)]
#[command(name = "codedb", about = "Code indexing and search")]
struct Cli {
    #[arg(long, default_value = "~/.codedb")]
    root: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Clone and index a git repository
    Index {
        /// Repository URL
        url: String,
    },
    /// Search indexed code
    Search {
        /// Search query
        query: String,
    },
    /// Run raw SQL query
    Sql {
        /// SQL query string
        query: String,
    },
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(stripped) = path.strip_prefix("~/") {
        if let Some(home) = dirs_next::home_dir() {
            return home.join(stripped);
        }
    }
    PathBuf::from(path)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = expand_tilde(&cli.root);

    match cli.command {
        Commands::Index { url } => {
            let mut db = CodeDB::open(&root)?;
            println!("Indexing {}...", url);
            db.index_repo(&url)?;
            println!("Done.");
        }
        Commands::Search { query } => {
            let db = CodeDB::open(&root)?;
            let mut stmt = db.conn().prepare(
                "SELECT fr.path, cs.score, cs.snippet
                 FROM code_search(?1) cs
                 JOIN blobs b ON b.id = cs.blob_id
                 JOIN file_revs fr ON fr.blob_id = b.id
                 JOIN refs r ON r.commit_id = fr.commit_id
                 GROUP BY fr.path
                 ORDER BY cs.score DESC
                 LIMIT 20"
            )?;
            let results = stmt.query_map([&query], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, f64>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in results {
                let (path, score, snippet) = row?;
                println!("{} (score: {:.2})", path, score);
                println!("  {}", snippet);
                println!();
            }
        }
        Commands::Sql { query } => {
            let db = CodeDB::open(&root)?;
            let mut stmt = db.conn().prepare(&query)?;
            let col_count = stmt.column_count();
            let col_names: Vec<String> = (0..col_count)
                .map(|i| stmt.column_name(i).unwrap().to_string())
                .collect();
            println!("{}", col_names.join("\t"));

            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let vals: Vec<String> = (0..col_count)
                    .map(|i| {
                        row.get::<_, rusqlite::types::Value>(i)
                            .map(|v| format!("{:?}", v))
                            .unwrap_or_else(|_| "NULL".to_string())
                    })
                    .collect();
                println!("{}", vals.join("\t"));
            }
        }
    }

    Ok(())
}
