use std::path::PathBuf;
use anyhow::Result;
use clap::{Parser, Subcommand};
use codedb_core::{CodeDB, SearchType};

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

        /// Maximum number of commits to walk per ref (0 = unlimited)
        #[arg(long, default_value = "10000")]
        depth: usize,
    },
    /// Search indexed code using Sourcegraph query syntax
    ///
    /// Supports filters: repo:, file:, -file:, lang:, type:, rev:, count:,
    /// author:, before:, after:, message:, select:
    ///
    /// Examples:
    ///   codedb search "process_data"
    ///   codedb search "lang:rust file:*.rs process_data"
    ///   codedb search "type:symbol lang:rust SFrame"
    ///   codedb search "type:diff author:ylow streaming"
    ///   codedb search "type:commit before:2026-01-01 refactor"
    Search {
        /// Search query (Sourcegraph syntax)
        query: String,

        /// Print generated SQL instead of executing
        #[arg(long)]
        sql: bool,
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
        Commands::Index { url, depth } => {
            let mut db = CodeDB::open(&root)?;
            let progress = |msg: &str| {
                eprintln!("{msg}");
            };
            let max_depth = if depth == 0 { None } else { Some(depth) };
            db.index_repo(&url, Some(&progress), max_depth)?;
            let stats = db.parse_symbols(Some(&progress))?;
            eprintln!(
                "Done. Parsed {} blobs, extracted {} symbols.",
                stats.blobs_parsed, stats.symbols_extracted
            );
        }
        Commands::Search { query, sql: show_sql } => {
            let db = CodeDB::open(&root)?;

            if show_sql {
                let translated = db.translate_query(&query)?;
                println!("-- Sourcegraph query: {query}");
                println!("-- Parameters: {:?}", translated.params);
                println!("{}", translated.sql);
                return Ok(());
            }

            let results = db.search(&query)?;

            if results.rows.is_empty() {
                println!("No results found.");
                return Ok(());
            }

            for row in &results.rows {
                match results.search_type {
                    SearchType::Code => {
                        let path = &row.columns[0].1;
                        let score = &row.columns[1].1;
                        let snippet = &row.columns[2].1;
                        println!("{path} (score: {score})");
                        println!("  {snippet}");
                        println!();
                    }
                    SearchType::Diff => {
                        let hash = &row.columns[0].1;
                        let message = &row.columns[1].1;
                        let path = &row.columns[2].1;
                        let score = &row.columns[3].1;
                        println!("{hash} {path} (score: {score})");
                        println!("  {message}");
                        println!();
                    }
                    SearchType::Commit => {
                        let hash = &row.columns[0].1;
                        let author = &row.columns[1].1;
                        let message = &row.columns[2].1;
                        println!("{hash} ({author}) {message}");
                    }
                    SearchType::Symbol => {
                        let path = &row.columns[0].1;
                        let name = &row.columns[1].1;
                        let kind = &row.columns[2].1;
                        let line = &row.columns[3].1;
                        println!("{path}:{line} {kind} {name}");
                    }
                }
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
                            .map(|v| format!("{v:?}"))
                            .unwrap_or_else(|_| "NULL".to_string())
                    })
                    .collect();
                println!("{}", vals.join("\t"));
            }
        }
    }

    Ok(())
}
