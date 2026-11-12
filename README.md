# jarscope

Fast JAR inspector for the command line. Answers questions about Java JAR files
instantly, without starting a JVM: what Java a JAR needs, what is inside it, and which
JAR in a whole folder contains a class.

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

### `jarscope find <class> <dir>`

Finds which JARs in a folder contain a class. It walks `<dir>` recursively and searches
the JARs in parallel, reading only each ZIP central directory, so nothing is decompressed.
`<dir>` can also be a single JAR. Results are sorted by path.

```
$ jarscope find org.slf4j.Logger ~/.m2
~/.m2/repository/org/slf4j/slf4j-api/2.0.17/slf4j-api-2.0.17.jar
~/.m2/wrapper/dists/apache-maven-3.9.11-bin/6mqf5t809d9geo83kj4ttckcbc/apache-maven-3.9.11/lib/slf4j-api-1.7.36.jar
…
7 matches in 800 jars (0.25 s)
```

The summary goes to stderr, so stdout is only the paths.

**Names.** `org.slf4j.Logger`, `org/slf4j/Logger` and `org/slf4j/Logger.class` are the
same query. A nested class can be written `Outer.Inner`, which also matches
`Outer$Inner.class`.

**Patterns.** `*` matches within one package segment; `**` matches across packages.
A pattern lists the matching classes under each JAR:

```
$ jarscope find 'org.slf4j.*Factory' ~/.m2
~/.m2/repository/org/slf4j/slf4j-api/2.0.17/slf4j-api-2.0.17.jar
  org.slf4j.ILoggerFactory
  org.slf4j.IMarkerFactory
  org.slf4j.LoggerFactory
  org.slf4j.MarkerFactory
…
7 matches in 800 jars (0.25 s)
```

Other examples: `'com.foo.*Service'` (one package), `'**.Logger'` (a `Logger` in any
package), `'com.foo.**'` (everything under `com.foo`).

**Where classes are found.** Besides the JAR root, `find` matches `META-INF/versions/N/`,
`BOOT-INF/classes/` and `WEB-INF/classes/`. In pattern mode the location is shown as a
suffix, like `module-info (META-INF/versions/9)`. `find` never reads the manifest, so
versioned classes match even without `Multi-Release: true`.

**`--nested`** also searches JARs inside JARs (`META-INF/jars/`, `META-INF/jarjar/`,
`BOOT-INF/lib/`, `WEB-INF/lib/`), as used by Fabric, Spring Boot and WAR files. An inner
JAR is shown as `outer.jar!/inner.jar`. Inner JARs are read into memory, so there are
limits on size, depth and the total per JAR; when one is hit, a warning is printed instead
of silently dropping results.

```
$ jarscope find --nested com.llamalad7.mixinextras.MixinExtrasBootstrap ~/Library/Application\ Support/minecraft
~/Library/Application Support/minecraft/.fabric/processedMods/mixinextras-0.4.1-215bdc40eef77b8f.jar
~/Library/Application Support/minecraft/libraries/net/fabricmc/fabric-loader/0.16.9/fabric-loader-0.16.9.jar!/META-INF/jars/mixinextras-fabric-0.4.1.jar
2 matches in 94 jars (0.16 s)
```

### Exit codes

`version`, `list` and `info`:

| Code | Meaning |
|---|---|
| 0 | Success (also when some classes are invalid; they are listed) |
| 2 | Error: file missing, not a JAR/ZIP, empty or truncated |

`find`:

| Code | Meaning |
|---|---|
| 0 | At least one match |
| 1 | No match |
| 2 | Error: empty query, path missing or unreadable, or a single file that isn't a JAR |

Warnings (for example, a JAR with duplicate entry names) go to stderr. In a folder scan, a
bad JAR is warned about and skipped, and the scan continues.

## Performance

Measured with [hyperfine](https://github.com/sharkdp/hyperfine) on an Apple M4,
Guava 33.5.0 (1,961 classes):

| Command | Time | Peak memory |
|---|---|---|
| `jarscope version guava.jar` (all classes) | 9.4 ms | 2.3 MiB |
| `jarscope list guava.jar` | 1.7 ms | 2.6 MiB |
| `jarscope info guava.jar` | 2.0 ms | 2.6 MiB |
| `jarscope find <exact name> guava.jar` | 1.7 ms | 3.2 MiB |
| `unzip -Z1 guava.jar` (list) | 3.1 ms | 2.3 MiB |
| `unzip -v guava.jar` (info) | 4.9 ms | 2.4 MiB |
| `jar tf guava.jar` | 45.3 ms | 42.1 MiB |
| `javap -v` on one class | 144 ms | 73 MiB |
| `javap -v` on all 1,961 classes | 1.5 s | 193 MiB |

Finding `org.slf4j.Logger` in the 800 JARs of `~/.m2` (7 hits):

| Command | Time |
|---|---|
| `jarscope find org.slf4j.Logger ~/.m2` | 24 ms ± 1 ms |
| `jarscope find --nested org.slf4j.Logger ~/.m2` | 24 ms ± 1 ms |
| loop of `unzip -Z1` per JAR | 3.4 s (0.84 s with 10 in parallel) |
| loop of `jar tf` per JAR | 33 s |

On `~/.gradle` (1,626 JARs among 35k files) `find` takes about 0.26 s; about 0.22 s of that is walking the folder.

jarscope is faster than `unzip` on time everywhere except tiny-JAR startup (+0.2 ms). Memory is equal on normal JARs, but `unzip` streams the central directory and uses far less on huge JARs (2.4 MiB vs about 20 MiB for 50k entries).

Checking 800 JARs, one process per JAR: `jarscope version` takes 1.7 s; `jar tf` (which
only lists entries) takes 30 s.

The release binary is about 1.0 MiB.

## Roadmap

- `--json` output for every command
- Minecraft mod metadata (Fabric, Quilt, Forge, NeoForge)

## License

MIT
