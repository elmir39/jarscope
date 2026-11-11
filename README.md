# jarscope

Fast JAR inspector for the command line. Answers questions about Java JAR files
instantly, without starting a JVM.

```
$ jarscope version guava-33.5.0-jre.jar
Requires: Java 8 (major 52)

Classes by Java version:
  Java 8  1961

module-info.class: Java 9 (META-INF/versions/9/module-info.class)
```

## Install

```sh
cargo install --locked jarscope
```

Requires Rust 1.88 or newer. No Java installation is needed to run it.

## Usage

### `jarscope version <jar>`

Shows the minimum Java version a JAR needs, and how many classes target each version.

It reads only the first 8 bytes of each class file, so it handles JARs with thousands of
classes in milliseconds. It also handles the cases where a naive check gives the wrong
answer:

| Case | How it's reported |
|---|---|
| Multi-release JARs (`META-INF/versions/N/`) | Listed per release, not counted toward the base requirement |
| `META-INF/versions/` without `Multi-Release: true` | Reported as ignored, as the JVM ignores them |
| `module-info.class` | Listed separately (Java 8 libraries often ship a Java 9 one) |
| Preview classes (`--enable-preview`) | Flagged: they run only on that exact Java version |
| Corrupt or truncated class files | Listed with the reason; the rest of the JAR is still checked |

A multi-release JAR with a preview class:

```
$ jarscope version app.jar
Requires: Java 17 (major 61)

Classes by Java version:
  Java 8   120
  Java 17    3

Multi-release (Multi-Release: true):
  META-INF/versions/21/   4 classes (Java 21)

Preview classes: 1 — need exactly Java 17 with --enable-preview
  com/example/P.class
```

### `jarscope list <jar>`

Lists the entries of a JAR, one name per line like `jar tf`, with each directory's entries
together. It reads only the ZIP central directory, so nothing is decompressed.

```
$ jarscope list guava-33.5.0-jre.jar | head -8
META-INF/
META-INF/LICENSE
META-INF/MANIFEST.MF
META-INF/maven/
META-INF/maven/com.google.guava/
META-INF/maven/com.google.guava/guava/
META-INF/maven/com.google.guava/guava/pom.properties
META-INF/maven/com.google.guava/guava/pom.xml
```

`--classes-only` prints only classes, as dotted names. Classes under
`META-INF/versions/N/` get a `(META-INF/versions/N)` suffix:

```
$ jarscope list guava-33.5.0-jre.jar --classes-only | tail -2
com.google.thirdparty.publicsuffix.TrieParser
module-info (META-INF/versions/9)
```

### `jarscope info <jar>`

Shows selected manifest fields, entry and class counts, and sizes. Only the manifest is
decompressed.

```
$ jarscope info guava-33.5.0-jre.jar
Created-By:      Apache Maven Bundle Plugin 5.1.9
Build-Jdk-Spec:  11
Multi-Release:   true
Entries:         2,002 (1,962 classes)
Compressed:      2.6 MB (2,610,717 bytes)
Uncompressed:    6.7 MB (6,727,760 bytes)
```

### Exit codes

The same for `version`, `list` and `info`. Warnings (for example, a JAR with duplicate
entry names) go to stderr.

| Code | Meaning |
|---|---|
| 0 | Success (also when some classes are invalid; they are listed) |
| 2 | Error: file missing, not a JAR/ZIP, empty or truncated |

## Performance

Measured with [hyperfine](https://github.com/sharkdp/hyperfine) on an Apple M4,
Guava 33.5.0 (1,961 classes):

| Command | Time | Peak memory |
|---|---|---|
| `jarscope version guava.jar` (all classes) | 10 ms | 2.3 MiB |
| `jarscope list guava.jar` | 2.9 ms | 2.6 MiB |
| `jarscope info guava.jar` | 2.8 ms | — |
| `jar tf guava.jar` | 45.3 ms | 42.1 MiB |
| `javap -v` on one class | 144 ms | 73 MiB |
| `javap -v` on all 1,961 classes | 1.5 s | 193 MiB |

Checking 800 JARs, one process per JAR: `jarscope version` takes 1.7 s; `jar tf` (which
only lists entries) takes 30 s.

The release binary is about 830 KiB.

## Roadmap

- `find`: find which JARs in a folder contain a class, scanning in parallel
- `--json` output for every command
- Minecraft mod metadata (Fabric, Quilt, Forge, NeoForge)

## License

MIT
