use anyhow::{bail, Context, Result};
use std::path::Path;

/// A parsed Dockerfile instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Instruction {
    /// `FROM [--platform=<p>] <image> [AS <alias>]`
    From {
        image: String,
        platform: Option<String>,
        alias: Option<String>,
    },
    /// `RUN <command>` (exec array or shell string)
    Run(CommandForm),
    /// `COPY [--from=<stage>] <sources...> <dest>`
    Copy {
        sources: Vec<String>,
        dest: String,
        from_stage: Option<String>,
        chown: Option<String>,
    },
    /// `ADD <sources...> <dest>`
    Add {
        sources: Vec<String>,
        dest: String,
        chown: Option<String>,
    },
    /// `WORKDIR <path>`
    Workdir(String),
    /// `ENV <key>=<val>` or `ENV <key> <val>`
    Env(Vec<(String, String)>),
    /// `CMD <command>`
    Cmd(CommandForm),
    /// `ENTRYPOINT <command>`
    Entrypoint(CommandForm),
    /// `EXPOSE <port>[/<protocol>]`
    Expose(Vec<String>),
    /// `USER <user>[:<group>]`
    User(String),
    /// `LABEL <key>=<val> ...`
    Label(Vec<(String, String)>),
}

/// Command form: either JSON array `["executable", "param1", ...]` or shell string `"echo foo"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandForm {
    Exec(Vec<String>),
    Shell(String),
}

impl CommandForm {
    pub fn to_args(&self) -> Vec<String> {
        match self {
            CommandForm::Exec(args) => args.clone(),
            CommandForm::Shell(sh) => vec!["/bin/sh".to_string(), "-c".to_string(), sh.clone()],
        }
    }
}

/// A parsed Dockerfile containing one or more instructions.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Dockerfile {
    pub instructions: Vec<Instruction>,
}

impl Dockerfile {
    /// Parses a Dockerfile from a raw string.
    pub fn parse(content: &str) -> Result<Self> {
        let logical_lines = join_continuation_lines(content);
        let mut instructions = Vec::new();

        for (line_num, line) in logical_lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }

            let instruction = parse_instruction(trimmed).with_context(|| {
                format!(
                    "Failed to parse Dockerfile line {}: '{}'",
                    line_num + 1,
                    trimmed
                )
            })?;
            instructions.push(instruction);
        }

        if instructions.is_empty() {
            bail!("Dockerfile contains no instructions");
        }

        // Validate that first instruction is FROM
        if !matches!(instructions.first(), Some(Instruction::From { .. })) {
            bail!("First instruction in Dockerfile must be 'FROM'");
        }

        Ok(Dockerfile { instructions })
    }

    /// Reads and parses a Dockerfile from disk.
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path.as_ref())
            .with_context(|| format!("Failed to read Dockerfile at {}", path.as_ref().display()))?;
        Self::parse(&content)
    }
}

/// Joins lines terminating with backslash `\` into single logical lines.
fn join_continuation_lines(content: &str) -> Vec<String> {
    let mut logical_lines = Vec::new();
    let mut current_line = String::new();

    for raw_line in content.lines() {
        let trimmed_end = raw_line.trim_end();
        if let Some(stripped) = trimmed_end.strip_suffix('\\') {
            current_line.push_str(stripped.trim_end());
            current_line.push(' ');
        } else {
            current_line.push_str(raw_line);
            logical_lines.push(current_line);
            current_line = String::new();
        }
    }

    if !current_line.is_empty() {
        logical_lines.push(current_line);
    }

    logical_lines
}

fn parse_instruction(line: &str) -> Result<Instruction> {
    let (directive, rest) = match line.split_once(char::is_whitespace) {
        Some((d, r)) => (d.trim(), r.trim()),
        None => (line.trim(), ""),
    };

    match directive.to_uppercase().as_str() {
        "FROM" => parse_from(rest),
        "RUN" => parse_run(rest),
        "COPY" => parse_copy(rest),
        "ADD" => parse_add(rest),
        "WORKDIR" => parse_workdir(rest),
        "ENV" => parse_env(rest),
        "CMD" => parse_cmd(rest),
        "ENTRYPOINT" => parse_entrypoint(rest),
        "EXPOSE" => parse_expose(rest),
        "USER" => parse_user(rest),
        "LABEL" => parse_label(rest),
        unknown => bail!("Unsupported Dockerfile instruction: '{}'", unknown),
    }
}

fn parse_from(args: &str) -> Result<Instruction> {
    let parts: Vec<&str> = args.split_whitespace().collect();
    if parts.is_empty() {
        bail!("FROM instruction requires an image name");
    }

    let mut platform = None;
    let mut image = None;
    let mut alias = None;

    let mut idx = 0;
    while idx < parts.len() {
        let part = parts[idx];
        if let Some(p) = part.strip_prefix("--platform=") {
            platform = Some(p.to_string());
            idx += 1;
        } else if image.is_none() {
            image = Some(part.to_string());
            idx += 1;
        } else if part.eq_ignore_ascii_case("AS") {
            if idx + 1 >= parts.len() {
                bail!("FROM ... AS requires a stage alias name");
            }
            alias = Some(parts[idx + 1].to_string());
            idx += 2;
        } else {
            idx += 1;
        }
    }

    let image = image.ok_or_else(|| anyhow::anyhow!("Missing image name in FROM instruction"))?;

    Ok(Instruction::From {
        image,
        platform,
        alias,
    })
}

fn parse_command_form(args: &str) -> CommandForm {
    let trimmed = args.trim();
    if trimmed.starts_with('[') && trimmed.ends_with(']') {
        if let Ok(parsed) = serde_json::from_str::<Vec<String>>(trimmed) {
            return CommandForm::Exec(parsed);
        }
    }
    CommandForm::Shell(trimmed.to_string())
}

fn parse_run(args: &str) -> Result<Instruction> {
    if args.trim().is_empty() {
        bail!("RUN instruction requires a command");
    }
    Ok(Instruction::Run(parse_command_form(args)))
}

fn parse_cmd(args: &str) -> Result<Instruction> {
    if args.trim().is_empty() {
        bail!("CMD instruction requires a command");
    }
    Ok(Instruction::Cmd(parse_command_form(args)))
}

fn parse_entrypoint(args: &str) -> Result<Instruction> {
    if args.trim().is_empty() {
        bail!("ENTRYPOINT instruction requires a command");
    }
    Ok(Instruction::Entrypoint(parse_command_form(args)))
}

fn parse_workdir(args: &str) -> Result<Instruction> {
    let wd = args.trim();
    if wd.is_empty() {
        bail!("WORKDIR instruction requires a path");
    }
    Ok(Instruction::Workdir(wd.to_string()))
}

fn parse_user(args: &str) -> Result<Instruction> {
    let user = args.trim();
    if user.is_empty() {
        bail!("USER instruction requires a username or UID");
    }
    Ok(Instruction::User(user.to_string()))
}

fn parse_expose(args: &str) -> Result<Instruction> {
    let ports = args.split_whitespace().map(|s| s.to_string()).collect();
    Ok(Instruction::Expose(ports))
}

fn parse_copy(args: &str) -> Result<Instruction> {
    let tokens: Vec<&str> = args.split_whitespace().collect();
    if tokens.len() < 2 {
        bail!("COPY instruction requires at least one source and a destination");
    }

    let mut from_stage = None;
    let mut chown = None;
    let mut paths = Vec::new();

    for token in tokens {
        if let Some(stage) = token.strip_prefix("--from=") {
            from_stage = Some(stage.to_string());
        } else if let Some(c) = token.strip_prefix("--chown=") {
            chown = Some(c.to_string());
        } else {
            paths.push(token.to_string());
        }
    }

    if paths.len() < 2 {
        bail!("COPY requires at least one source and a destination");
    }

    let dest = paths.pop().unwrap();
    let sources = paths;

    Ok(Instruction::Copy {
        sources,
        dest,
        from_stage,
        chown,
    })
}

fn parse_add(args: &str) -> Result<Instruction> {
    let tokens: Vec<&str> = args.split_whitespace().collect();
    if tokens.len() < 2 {
        bail!("ADD instruction requires at least one source and a destination");
    }

    let mut chown = None;
    let mut paths = Vec::new();

    for token in tokens {
        if let Some(c) = token.strip_prefix("--chown=") {
            chown = Some(c.to_string());
        } else {
            paths.push(token.to_string());
        }
    }

    if paths.len() < 2 {
        bail!("ADD requires at least one source and a destination");
    }

    let dest = paths.pop().unwrap();
    let sources = paths;

    Ok(Instruction::Add {
        sources,
        dest,
        chown,
    })
}

fn parse_env(args: &str) -> Result<Instruction> {
    let trimmed = args.trim();
    if trimmed.is_empty() {
        bail!("ENV instruction requires arguments");
    }

    // Two forms:
    // 1. ENV KEY=VAL KEY2=VAL2 ...
    // 2. ENV KEY VAL
    let mut pairs = Vec::new();

    if trimmed.contains('=') {
        // Form 1
        for part in trimmed.split_whitespace() {
            if let Some((k, v)) = part.split_once('=') {
                let clean_v = v.trim_matches('"').trim_matches('\'');
                pairs.push((k.to_string(), clean_v.to_string()));
            }
        }
    } else {
        // Form 2
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        if parts.len() < 2 {
            bail!("Invalid ENV instruction format: '{}'", trimmed);
        }
        let key = parts[0].to_string();
        let val = parts[1..].join(" ");
        pairs.push((key, val));
    }

    if pairs.is_empty() {
        bail!(
            "Failed to parse any environment variables from: '{}'",
            trimmed
        );
    }

    Ok(Instruction::Env(pairs))
}

fn parse_label(args: &str) -> Result<Instruction> {
    let trimmed = args.trim();
    let mut pairs = Vec::new();

    for part in trimmed.split_whitespace() {
        if let Some((k, v)) = part.split_once('=') {
            let clean_v = v.trim_matches('"').trim_matches('\'');
            pairs.push((k.to_string(), clean_v.to_string()));
        }
    }

    Ok(Instruction::Label(pairs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_dockerfile() {
        let df_str = r#"
        # Sample Dockerfile
        FROM alpine:3.19
        WORKDIR /app
        COPY . /app
        RUN echo "building" \
            && ls -la
        ENV PORT=8080 ENV_NAME=production
        EXPOSE 8080 9000
        CMD ["/app/bin", "--start"]
        "#;

        let df = Dockerfile::parse(df_str).unwrap();
        assert_eq!(df.instructions.len(), 7);

        assert_eq!(
            df.instructions[0],
            Instruction::From {
                image: "alpine:3.19".to_string(),
                platform: None,
                alias: None,
            }
        );

        assert_eq!(df.instructions[1], Instruction::Workdir("/app".to_string()));

        assert_eq!(
            df.instructions[2],
            Instruction::Copy {
                sources: vec![".".to_string()],
                dest: "/app".to_string(),
                from_stage: None,
                chown: None,
            }
        );

        match &df.instructions[3] {
            Instruction::Run(CommandForm::Shell(cmd)) => {
                assert!(cmd.contains("building"));
                assert!(cmd.contains("ls -la"));
            }
            _ => panic!("Expected Run(Shell)"),
        }

        assert_eq!(
            df.instructions[4],
            Instruction::Env(vec![
                ("PORT".to_string(), "8080".to_string()),
                ("ENV_NAME".to_string(), "production".to_string()),
            ])
        );

        assert_eq!(
            df.instructions[5],
            Instruction::Expose(vec!["8080".to_string(), "9000".to_string()])
        );

        assert_eq!(
            df.instructions[6],
            Instruction::Cmd(CommandForm::Exec(vec![
                "/app/bin".to_string(),
                "--start".to_string()
            ]))
        );
    }

    #[test]
    fn test_parse_multi_stage_dockerfile() {
        let df_str = r#"
        FROM rust:1.75 AS builder
        WORKDIR /build
        COPY Cargo.toml .
        RUN cargo build --release

        FROM debian:bookworm-slim
        WORKDIR /usr/local/bin
        COPY --from=builder /build/target/release/app .
        ENTRYPOINT ["./app"]
        "#;

        let df = Dockerfile::parse(df_str).unwrap();
        assert_eq!(df.instructions.len(), 8);

        assert_eq!(
            df.instructions[0],
            Instruction::From {
                image: "rust:1.75".to_string(),
                platform: None,
                alias: Some("builder".to_string()),
            }
        );

        assert_eq!(
            df.instructions[6],
            Instruction::Copy {
                sources: vec!["/build/target/release/app".to_string()],
                dest: ".".to_string(),
                from_stage: Some("builder".to_string()),
                chown: None,
            }
        );
    }

    #[test]
    fn test_parse_env_key_space_val() {
        let df_str = "FROM alpine\nENV APP_ENV production";
        let df = Dockerfile::parse(df_str).unwrap();
        assert_eq!(
            df.instructions[1],
            Instruction::Env(vec![("APP_ENV".to_string(), "production".to_string())])
        );
    }

    #[test]
    fn test_parse_empty_fails() {
        assert!(Dockerfile::parse("").is_err());
        assert!(Dockerfile::parse("# only comments").is_err());
    }

    #[test]
    fn test_parse_non_from_first_fails() {
        assert!(Dockerfile::parse("RUN echo fail\nFROM alpine").is_err());
    }
}
