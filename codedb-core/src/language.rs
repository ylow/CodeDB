/// Detect programming language from file path extension.
pub fn detect_language(path: &str) -> Option<&'static str> {
    let ext = path.rsplit('.').next()?;
    match ext.to_ascii_lowercase().as_str() {
        "rs" => Some("rust"),
        "py" => Some("python"),
        "js" => Some("javascript"),
        "ts" => Some("typescript"),
        "tsx" => Some("tsx"),
        "jsx" => Some("jsx"),
        "java" => Some("java"),
        "c" => Some("c"),
        "h" => Some("c"),
        "cpp" | "cc" | "cxx" => Some("cpp"),
        "hpp" | "hxx" | "hh" => Some("cpp"),
        "go" => Some("go"),
        "rb" => Some("ruby"),
        "php" => Some("php"),
        "swift" => Some("swift"),
        "kt" | "kts" => Some("kotlin"),
        "scala" => Some("scala"),
        "cs" => Some("csharp"),
        "sh" | "bash" => Some("shell"),
        "sql" => Some("sql"),
        "html" | "htm" => Some("html"),
        "css" => Some("css"),
        "json" => Some("json"),
        "yaml" | "yml" => Some("yaml"),
        "toml" => Some("toml"),
        "xml" => Some("xml"),
        "md" | "markdown" => Some("markdown"),
        "r" => Some("r"),
        "lua" => Some("lua"),
        "zig" => Some("zig"),
        "ex" | "exs" => Some("elixir"),
        "erl" | "hrl" => Some("erlang"),
        "hs" => Some("haskell"),
        "ml" | "mli" => Some("ocaml"),
        "pl" | "pm" => Some("perl"),
        "proto" => Some("protobuf"),
        "dart" => Some("dart"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_language() {
        assert_eq!(detect_language("src/main.rs"), Some("rust"));
        assert_eq!(detect_language("lib.py"), Some("python"));
        assert_eq!(detect_language("Cargo.toml"), Some("toml"));
        assert_eq!(detect_language("README.md"), Some("markdown"));
        assert_eq!(detect_language("Makefile"), None);
        assert_eq!(detect_language("foo.bar.rs"), Some("rust"));
    }
}
