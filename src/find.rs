use std::fmt;
use std::io::{self, Cursor, Read, Seek};
use std::path::Path;

use anyhow::{Result, bail};
use zip::ZipArchive;

use crate::jar::{VERSIONS_PREFIX, open, parse_release};
use crate::render::human_size;

const BOOT_PREFIX: &str = "BOOT-INF/classes/";
const WEB_PREFIX: &str = "WEB-INF/classes/";
const NESTED_PREFIXES: [&str; 4] = [
    "META-INF/jars/",
    "META-INF/jarjar/",
    "BOOT-INF/lib/",
    "WEB-INF/lib/",
];

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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JarMatch {
    pub path: String,
    pub classes: Vec<ClassLocation>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct JarSearch {
    pub matches: Vec<JarMatch>,
    pub warnings: Vec<String>,
}

pub fn search_jar(path: &Path, query: &Query, nested: bool) -> Result<JarSearch> {
    let mut archive = open(path)?;
    let display = path.display().to_string();
    Ok(search(
        &mut archive,
        &display,
        query,
        nested,
        &Limits::DEFAULT,
    ))
}

struct Limits {
    size: u64,
    depth: usize,
    budget_bytes: u64,
    max_jars: usize,
}

impl Limits {
    const DEFAULT: Limits = Limits {
        // Nested JARs are read into memory and their declared size is untrusted.
        size: 256 << 20,
        // A crafted JAR can contain itself, so recursion needs a bound.
        depth: 3,
        // Per top-level JAR: overlapping central-directory entries can point thousands of
        // times at one tiny nested JAR on every level (fan-out cubed), and every level can be
        // a zip bomb; depth alone doesn't bound the work.
        budget_bytes: 1 << 30,
        max_jars: 10_000,
    };
}

#[derive(Default)]
struct Budget {
    bytes: u64,
    jars: usize,
    exhausted: bool,
}

struct Search<'a> {
    query: &'a Query,
    nested: bool,
    limits: &'a Limits,
    budget: Budget,
    out: JarSearch,
}

fn search<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    display: &str,
    query: &Query,
    nested: bool,
    limits: &Limits,
) -> JarSearch {
    let mut search = Search {
        query,
        nested,
        limits,
        budget: Budget::default(),
        out: JarSearch::default(),
    };
    search.archive(archive, display, 0);
    search.out
}

fn is_nested_jar(name: &str) -> bool {
    let bytes = name.as_bytes();
    NESTED_PREFIXES.iter().any(|p| name.starts_with(p))
        && bytes.len() >= 4
        && bytes[bytes.len() - 4..].eq_ignore_ascii_case(b".jar")
}

fn class_key(c: &ClassLocation) -> (Location, &str, &str) {
    let (package, simple) = c.name.rsplit_once('.').unwrap_or(("", &c.name));
    (c.location, package, simple)
}

impl Search<'_> {
    // Without `nested` no entry is opened: only the central directory is read.
    fn archive<R: Read + Seek>(
        &mut self,
        archive: &mut ZipArchive<R>,
        display: &str,
        depth: usize,
    ) {
        let mut classes = Vec::new();
        let mut candidates = Vec::new();
        for (i, name) in archive.file_names().enumerate() {
            let Ok(name) = name else { continue };
            if let Some(class) = class_location(&name) {
                if self.query.matches(&class.name) {
                    classes.push(class);
                }
            } else if self.nested && is_nested_jar(&name) {
                candidates.push((i, name.into_owned()));
            }
        }
        if !classes.is_empty() {
            classes.sort_by(|a, b| class_key(a).cmp(&class_key(b)));
            self.out.matches.push(JarMatch {
                path: display.to_string(),
                classes,
            });
        }
        candidates.sort_unstable_by(|a, b| a.1.cmp(&b.1));
        for (i, name) in candidates {
            let path = format!("{display}!/{name}");
            if let Some(mut inner) = self.open_nested(archive, i, &path, depth + 1) {
                self.archive(&mut inner, &path, depth + 1);
            }
        }
    }

    fn open_nested<R: Read + Seek>(
        &mut self,
        archive: &mut ZipArchive<R>,
        i: usize,
        path: &str,
        depth: usize,
    ) -> Option<ZipArchive<Cursor<Vec<u8>>>> {
        let limits = self.limits;
        if self.budget.exhausted {
            return None;
        }
        if depth > limits.depth {
            let msg = format!(
                "{path}: nested deeper than {} levels, not searched",
                limits.depth
            );
            return self.warn(msg);
        }
        let declared = match archive.by_index_data(i) {
            Ok(data) => data.size(),
            Err(e) => return self.warn(format!("cannot read {path}: {e}")),
        };
        if declared > limits.size {
            return self.warn(format!(
                "{path}: declared size {} is over the {} limit, not searched",
                human_size(declared),
                human_size(limits.size)
            ));
        }
        if self.budget.jars >= limits.max_jars {
            return self.exhaust(format!(
                "{path}: more than {} nested JARs, the rest are not searched",
                limits.max_jars
            ));
        }
        let remaining = limits.budget_bytes.saturating_sub(self.budget.bytes);
        if declared > remaining {
            return self.exhaust_bytes(path);
        }
        self.budget.jars += 1;

        let cap = limits.size.min(remaining);
        // The declared size is untrusted: don't reserve 256 MiB for an entry that holds a few bytes.
        let initial = declared.min(16 << 20);
        let mut bytes = Vec::with_capacity(usize::try_from(initial).unwrap_or(0));
        let read = archive
            .by_index(i)
            .map_err(io::Error::from)
            .and_then(|entry| entry.take(cap.saturating_add(1)).read_to_end(&mut bytes));
        self.budget.bytes = self.budget.bytes.saturating_add(bytes.len() as u64);
        if let Err(e) = read {
            return self.warn(format!("cannot read {path}: {e}"));
        }
        if bytes.len() as u64 > cap {
            return if cap == limits.size {
                self.warn(format!(
                    "{path}: over the {} limit, not searched",
                    human_size(limits.size)
                ))
            } else {
                self.exhaust_bytes(path)
            };
        }
        match ZipArchive::new(Cursor::new(bytes)) {
            Ok(inner) => Some(inner),
            Err(e) => self.warn(format!("{path} is not a valid JAR/ZIP file: {e}")),
        }
    }

    fn warn<T>(&mut self, msg: String) -> Option<T> {
        self.out.warnings.push(msg);
        None
    }

    fn exhaust<T>(&mut self, msg: String) -> Option<T> {
        self.budget.exhausted = true;
        self.warn(msg)
    }

    fn exhaust_bytes<T>(&mut self, path: &str) -> Option<T> {
        let limit = human_size(self.limits.budget_bytes);
        self.exhaust(format!(
            "{path}: nested JARs over {limit} in total, the rest are not searched"
        ))
    }
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

    fn jar(entries: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        use zip::write::SimpleFileOptions;
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    fn inner(n: usize) -> Vec<u8> {
        jar(&[(&format!("p{n}/X.class"), b"x")])
    }

    fn run(outer: Vec<u8>, limits: &Limits) -> (Vec<String>, Vec<String>) {
        let mut archive = ZipArchive::new(Cursor::new(outer)).unwrap();
        let out = search(&mut archive, "o.jar", &q("**.X"), true, limits);
        let paths = out.matches.into_iter().map(|m| m.path).collect();
        (paths, out.warnings)
    }

    const TINY: Limits = Limits {
        size: 1 << 20,
        depth: 3,
        budget_bytes: 1 << 20,
        max_jars: 100,
    };

    #[test]
    fn nested_over_declared_size_is_not_read() {
        let small = inner(0);
        let big = vec![0u8; 4000];
        let outer = jar(&[
            ("META-INF/jars/a.jar", &small),
            ("META-INF/jars/b.jar", &big),
        ]);
        let limits = Limits { size: 1000, ..TINY };
        let (paths, warnings) = run(outer, &limits);
        assert_eq!(paths, ["o.jar!/META-INF/jars/a.jar"]);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].starts_with("o.jar!/META-INF/jars/b.jar: declared size 4.0 KB"),
            "{warnings:?}"
        );
    }

    #[test]
    fn nested_jar_count_budget() {
        let jars: Vec<_> = (0..4).map(inner).collect();
        let names: Vec<_> = (0..4).map(|n| format!("META-INF/jars/{n}.jar")).collect();
        let mut entries: Vec<(&str, &[u8])> = vec![("o/X.class", b"x")];
        entries.extend(
            names
                .iter()
                .map(String::as_str)
                .zip(jars.iter().map(Vec::as_slice)),
        );
        let limits = Limits {
            max_jars: 2,
            ..TINY
        };
        let (paths, warnings) = run(jar(&entries), &limits);
        assert_eq!(
            paths,
            [
                "o.jar",
                "o.jar!/META-INF/jars/0.jar",
                "o.jar!/META-INF/jars/1.jar"
            ]
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].starts_with("o.jar!/META-INF/jars/2.jar: more than 2 nested JARs"),
            "{warnings:?}"
        );
    }

    #[test]
    fn nested_byte_budget() {
        let jars: Vec<_> = (0..3).map(inner).collect();
        let size = jars[0].len() as u64;
        let outer = jar(&[
            ("META-INF/jars/0.jar", &jars[0]),
            ("META-INF/jars/1.jar", &jars[1]),
            ("META-INF/jars/2.jar", &jars[2]),
        ]);
        let limits = Limits {
            budget_bytes: size + size / 2,
            ..TINY
        };
        let (paths, warnings) = run(outer, &limits);
        assert_eq!(paths, ["o.jar!/META-INF/jars/0.jar"]);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].starts_with("o.jar!/META-INF/jars/1.jar: nested JARs over"),
            "{warnings:?}"
        );
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
