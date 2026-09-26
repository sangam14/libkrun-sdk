use std::path::Path;

/// Dockerignore pattern matcher.
#[derive(Debug, Clone, Default)]
pub struct Dockerignore {
    rules: Vec<IgnoreRule>,
}

#[derive(Debug, Clone)]
struct IgnoreRule {
    pattern: String,
    negated: bool,
    dir_only: bool,
}

impl Dockerignore {
    /// Loads a `.dockerignore` file if present in the given context directory.
    pub fn load_from_context<P: AsRef<Path>>(context_dir: P) -> Self {
        let ignore_file = context_dir.as_ref().join(".dockerignore");
        if ignore_file.exists() {
            if let Ok(content) = std::fs::read_to_string(&ignore_file) {
                return Self::parse(&content);
            }
        }
        Self::default()
    }

    /// Parses `.dockerignore` file content.
    pub fn parse(content: &str) -> Self {
        let mut rules = Vec::new();
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            let (negated, pattern) = if let Some(stripped) = trimmed.strip_prefix('!') {
                (true, stripped.trim())
            } else {
                (false, trimmed)
            };

            let dir_only = pattern.ends_with('/');
            let clean = pattern
                .strip_prefix('/')
                .unwrap_or(pattern)
                .trim_end_matches('/')
                .to_string();

            rules.push(IgnoreRule {
                pattern: clean,
                negated,
                dir_only,
            });
        }

        Self { rules }
    }

    /// Determines if a relative file path within the context should be ignored.
    pub fn is_ignored<P: AsRef<Path>>(&self, rel_path: P) -> bool {
        let path_str = rel_path.as_ref().to_string_lossy().to_string();
        let normalized = path_str.strip_prefix("./").unwrap_or(&path_str);

        // Always ignore .git directory
        if normalized == ".git" || normalized.starts_with(".git/") {
            return true;
        }

        let mut ignored = false;
        for rule in &self.rules {
            let matched = if rule.dir_only {
                normalized == rule.pattern || normalized.starts_with(&format!("{}/", rule.pattern))
            } else {
                glob_match(&rule.pattern, normalized)
                    || (!rule.pattern.contains('/')
                        && normalized
                            .rsplit('/')
                            .next()
                            .is_some_and(|f| glob_match(&rule.pattern, f)))
                    || normalized == rule.pattern
                    || normalized.starts_with(&format!("{}/", rule.pattern))
            };

            if matched {
                ignored = !rule.negated;
            }
        }

        ignored
    }
}

/// Simple glob matcher supporting `*`, `**`, and `?`.
fn glob_match(pattern: &str, path: &str) -> bool {
    let p_parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    match_parts(&p_parts, &path_parts)
}

fn match_parts(pattern_parts: &[&str], path_parts: &[&str]) -> bool {
    if pattern_parts.is_empty() {
        return path_parts.is_empty();
    }

    if pattern_parts[0] == "**" {
        if pattern_parts.len() == 1 {
            return true;
        }
        for i in 0..=path_parts.len() {
            if match_parts(&pattern_parts[1..], &path_parts[i..]) {
                return true;
            }
        }
        return false;
    }

    if path_parts.is_empty() {
        return false;
    }

    if match_segment(pattern_parts[0], path_parts[0]) {
        return match_parts(&pattern_parts[1..], &path_parts[1..]);
    }

    false
}

fn match_segment(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') && !pattern.contains('?') {
        return pattern == text;
    }

    let p_chars: Vec<char> = pattern.chars().collect();
    let t_chars: Vec<char> = text.chars().collect();

    let mut p_idx = 0;
    let mut t_idx = 0;
    let mut star_idx = None;
    let mut match_idx = 0;

    while t_idx < t_chars.len() {
        if p_idx < p_chars.len() && (p_chars[p_idx] == '?' || p_chars[p_idx] == t_chars[t_idx]) {
            p_idx += 1;
            t_idx += 1;
        } else if p_idx < p_chars.len() && p_chars[p_idx] == '*' {
            star_idx = Some(p_idx);
            match_idx = t_idx;
            p_idx += 1;
        } else if let Some(s_idx) = star_idx {
            p_idx = s_idx + 1;
            match_idx += 1;
            t_idx = match_idx;
        } else {
            return false;
        }
    }

    while p_idx < p_chars.len() && p_chars[p_idx] == '*' {
        p_idx += 1;
    }

    p_idx == p_chars.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dockerignore_rules() {
        let content = r#"
        # Comment
        target/
        *.log
        temp/**
        !important.log
        "#;

        let di = Dockerignore::parse(content);

        assert!(di.is_ignored(".git/HEAD"));
        assert!(di.is_ignored("target/debug/app"));
        assert!(di.is_ignored("debug.log"));
        assert!(di.is_ignored("src/nested/debug.log"));
        assert!(di.is_ignored("temp/foo/bar.txt"));
        assert!(!di.is_ignored("important.log"));
        assert!(!di.is_ignored("src/main.rs"));
    }
}
