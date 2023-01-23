use slog::Logger;
use std::{
    path::Path,
};
use structre::structre;
use crate::{
    bb,
    common::{
        Context,
        DEFAULT_WEIGHT,
        maybe_read,
    },
    o,
    warn,
};

pub fn process_golang_gomod(log: &Logger, ctx: &Context, path: &Path) {
    let path = path.join("go.mod");
    let log = log.new(o!(file = path.to_string_lossy().to_string()));

    #[structre(r#"^\s*(?P<keyword>[^\s]+)\s+(?P<remainder>.*)\s*$"#)]
    struct Keyword {
        keyword: String,
        remainder: String,
    }

    let parse_keyword = KeywordFromRegex::new();

    #[structre(r#"^\s*(?P<id>[^\s]+)\s+(?:[^\s]+)(?:\s+//\s+(?P<comment>[^\s]+))?\s*$"#)]
    struct Require {
        id: String,
        comment: String,
    }

    let parse_require = RequireFromRegex::new();
    let mut parens = 0;
    let mut in_require = false;
    let bytes = match maybe_read(&path) {
        Ok(None) => return,
        Err(e) => {
            warn!(log, "Error loading go.mod", err = format!("{:?}", e));
            return;
        },
        Ok(Some(b)) => b,
    };
    let lines = String::from_utf8_lossy(&bytes);
    let mut config = ctx.config.lock().unwrap();
    for line in lines.lines() {
        if parens == 0 {
            if let Ok(kw) = parse_keyword.parse(&line) {
                if kw.remainder.chars().next().unwrap_or(' ') == '(' {
                    parens += 1;
                    if kw.keyword == "require" {
                        in_require = true;
                    }
                } else if kw.keyword == "require" {
                    bb!({
                        let require = match parse_require.parse(&kw.remainder) {
                            Ok(require) => {
                                if require.comment == "indirect" {
                                    break;
                                }
                                require
                            },
                            Err(e) => {
                                warn!(log, "Error parsing require line", line = line, err = format!("{:?}", e));
                                break;
                            },
                        };
                        config.weights.projects.insert(format!("https://{}", require.id), DEFAULT_WEIGHT);
                    });
                }
            }
        } else {
            if line == ")" {
                parens -= 1;
                if in_require {
                    in_require = false;
                }
            } else if in_require {
                bb!({
                    let require = match parse_require.parse(&line) {
                        Ok(require) => {
                            if require.comment == "indirect" {
                                break;
                            }
                            require
                        },
                        Err(e) => {
                            warn!(log, "Error parsing require line", line = line, err = format!("{:?}", e));
                            break;
                        },
                    };
                    config.weights.projects.insert(format!("https://{}", require.id), DEFAULT_WEIGHT);
                });
            }
        }
    }
}
