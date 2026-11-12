use std::fmt;

use anyhow::{Result, bail};

use crate::jar::{VERSIONS_PREFIX, parse_release};

const BOOT_PREFIX: &str = "BOOT-INF/classes/";
const WEB_PREFIX: &str = "WEB-INF/classes/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Query {
    pattern: String,
    dotted: bool,
    has_wildcard: bool,
}

impl Query {
    pub fn parse(input: &str) -> Result<Query> {
        let trimmed = input.strip_suffix(".class").unwrap_or(input);
        if trimmed.is_empty() {
            bail!("empty class name");
        }
        let dotted = !trimmed.contains('/');
        Ok(Query {
            pattern: trimmed.replace('/', "."),
            dotted,
            has_wildcard: trimmed.contains('*'),
        })
    }

    pub fn is_pattern(&self) -> bool {
        self.has_wildcard
    }

    pub fn matches(&self, class_name: &str) -> bool {
        let pattern = self.pattern.as_bytes();
        glob(pattern, class_name.as_bytes())
            || (self.dotted
                && class_name.contains('$')
                && glob(pattern, class_name.replace('$', ".").as_bytes()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Location {
    Base,
    Versioned(u32),
    BootInf,
    WebInf,
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Location::Base => Ok(()),
            Location::Versioned(n) => write!(f, "{VERSIONS_PREFIX}{n}"),
            Location::BootInf => f.write_str("BOOT-INF/classes"),
            Location::WebInf => f.write_str("WEB-INF/classes"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassLocation {
    pub name: String,
    pub location: Location,
}

pub fn class_location(entry: &str) -> Option<ClassLocation> {
    let path = entry.strip_suffix(".class")?;
    if path.ends_with('/') {
        return None;
    }
    let versioned = path
        .strip_prefix(VERSIONS_PREFIX)
        .and_then(|rest| rest.split_once('/'))
        .and_then(|(n, rest)| Some((Location::Versioned(parse_release(n)?), rest)));
    let (location, path) = versioned
        .or_else(|| {
            path.strip_prefix(BOOT_PREFIX)
                .map(|r| (Location::BootInf, r))
        })
        .or_else(|| path.strip_prefix(WEB_PREFIX).map(|r| (Location::WebInf, r)))
        .unwrap_or((Location::Base, path));
    if path.is_empty() {
        return None;
    }
    Some(ClassLocation {
        name: path.replace('/', "."),
        location,
    })
}

#[derive(Clone, Copy)]
enum Token {
    Lit(u8),
    Star,
    DoubleStar,
}

fn tokenize(pattern: &[u8]) -> Vec<Token> {
    let mut tokens = Vec::with_capacity(pattern.len());
    let mut i = 0;
    while i < pattern.len() {
        if pattern[i] == b'*' {
            let run = pattern[i..].iter().take_while(|&&b| b == b'*').count();
            tokens.push(if run == 1 {
                Token::Star
            } else {
                Token::DoubleStar
            });
            i += run;
        } else {
            tokens.push(Token::Lit(pattern[i]));
            i += 1;
        }
    }
    tokens
}

// One rolling row, O(pattern * name): a greedy "last star" matcher is wrong with two wildcard kinds.
fn glob(pattern: &[u8], name: &[u8]) -> bool {
    let mut row = vec![false; name.len() + 1];
    row[0] = true;
    for token in tokenize(pattern) {
        match token {
            Token::Lit(c) => {
                for j in (1..=name.len()).rev() {
                    row[j] = row[j - 1] && name[j - 1] == c;
                }
                row[0] = false;
            }
            Token::Star => {
                for j in 1..=name.len() {
                    row[j] = row[j] || (row[j - 1] && name[j - 1] != b'.');
                }
            }
            Token::DoubleStar => {
                for j in 1..=name.len() {
                    row[j] = row[j] || row[j - 1];
                }
            }
        }
    }
    row[name.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(s: &str) -> Query {
        Query::parse(s).unwrap()
    }

    #[test]
    fn parse_normalizes() {
        let exact = q("com.foo.Bar");
        for s in ["com/foo/Bar", "com/foo/Bar.class", "com.foo.Bar.class"] {
            assert_eq!(q(s).pattern, exact.pattern);
            assert!(!q(s).is_pattern());
        }
        assert!(Query::parse("").is_err());
        assert!(Query::parse(".class").is_err());
        assert!(q("a.*").is_pattern());
    }

    #[test]
    fn exact_match() {
        let query = q("com.foo.Bar");
        assert!(query.matches("com.foo.Bar"));
        for s in ["com.foo.BarX", "xcom.foo.Bar", "com.foo.bar"] {
            assert!(!query.matches(s), "{s}");
        }
    }

    #[test]
    fn nested_class_forms() {
        let name = "com.foo.Outer$Inner";
        assert!(q("com.foo.Outer.Inner").matches(name));
        assert!(q("com.foo.Outer$Inner").matches(name));
        assert!(q("com/foo/Outer$Inner").matches(name));
        assert!(!q("com/foo/Outer/Inner").matches(name));
    }

    #[test]
    fn glob_matching() {
        let svc = q("com.foo.*Service");
        for s in [
            "com.foo.UserService",
            "com.foo.Service",
            "com.foo.Outer$InnerService",
        ] {
            assert!(svc.matches(s), "{s}");
        }
        for s in ["com.foo.bar.UserService", "com.foo.UserServiceImpl"] {
            assert!(!svc.matches(s), "{s}");
        }
        assert!(q("com.foo.**Service").matches("com.foo.bar.UserService"));
        assert!(q("**.Logger").matches("org.slf4j.Logger"));
        assert!(!q("**.Logger").matches("Logger"));
        assert!(q("**Logger").matches("Logger"));
        assert!(q("**Logger").matches("org.slf4j.Logger"));
        assert!(q("*").matches("Bar"));
        assert!(!q("*").matches("a.Bar"));
        for p in ["**", "***"] {
            assert!(q(p).matches("a.b.Bar"));
        }
        assert!(q("a*b*c").matches("aXbYc"));
        assert!(q("a*b*c").matches("abc"));
        assert!(!q("a*b*c").matches("acb"));
        assert!(q("com.foo.Outer.*").matches("com.foo.Outer$Inner"));
        assert!(!q("com.foo.**").matches("com.foo"));
    }

    #[test]
    fn pathological_pattern_is_fast() {
        let name = "a".repeat(10_000);
        assert!(!q("*a*a*a*a*a*a*a*b").matches(&name));
    }

    #[test]
    fn location_order() {
        assert!(Location::Base < Location::Versioned(9));
        assert!(Location::Versioned(9) < Location::Versioned(17));
        assert!(Location::Versioned(17) < Location::BootInf);
        assert!(Location::BootInf < Location::WebInf);
    }

    #[test]
    fn location_display() {
        assert_eq!(Location::Base.to_string(), "");
        assert_eq!(Location::Versioned(17).to_string(), "META-INF/versions/17");
        assert_eq!(Location::BootInf.to_string(), "BOOT-INF/classes");
        assert_eq!(Location::WebInf.to_string(), "WEB-INF/classes");
    }

    fn loc(entry: &str) -> Option<(String, Location)> {
        class_location(entry).map(|c| (c.name, c.location))
    }

    #[test]
    fn class_locations() {
        let cases = [
            ("com/foo/Bar.class", "com.foo.Bar", Location::Base),
            (
                "META-INF/versions/17/com/foo/Bar.class",
                "com.foo.Bar",
                Location::Versioned(17),
            ),
            (
                "BOOT-INF/classes/com/foo/Bar.class",
                "com.foo.Bar",
                Location::BootInf,
            ),
            (
                "WEB-INF/classes/com/foo/Bar.class",
                "com.foo.Bar",
                Location::WebInf,
            ),
            (
                "META-INF/versions/8/X.class",
                "META-INF.versions.8.X",
                Location::Base,
            ),
            (
                "META-INF/versions/9/module-info.class",
                "module-info",
                Location::Versioned(9),
            ),
        ];
        for (entry, name, location) in cases {
            assert_eq!(loc(entry), Some((name.to_string(), location)), "{entry}");
        }
        for entry in ["BOOT-INF/classes/", "README.md", "x.class/"] {
            assert_eq!(loc(entry), None, "{entry}");
        }
    }
}
